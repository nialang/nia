// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    ActiveModuleItemTreeFactKind, CheckedModule, CheckedProgram, CheckedProgramAnalysis,
    CodegenPreparation, CodegenProgram, FrontendCheckCertificateCacheKey,
    FrontendCheckInputFingerprint, FrontendCheckScope, ProgramDiagnostic, RuntimeModel, TimingMode,
    module_diagnostics,
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
use nia_source::{SourceIdentity, SourcePath, SourceVersion};
use nia_span::Span;
use nia_symbol::SymbolId;
use nia_target_config::TargetConfig;
use nia_ty::{ArrayLenTy, TyKind};
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_type_resolve::TypeResolution;
use nia_value_resolve::ValueResolution;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, RwLock},
};

mod backend_lowering;
mod base;
mod checked;
mod checks;
mod diagnostics;
mod executable;
mod extension_provider_queries;
mod function_body_queries;
mod program;
mod program_signature_queries;
mod providers;
mod resolve;
mod static_init_queries;
mod types;

use backend_lowering::*;
use base::*;
use checked::*;
use checks::*;
use diagnostics::*;
use executable::*;
use extension_provider_queries::*;
use function_body_queries::*;
use program::*;
use program_signature_queries::*;
use providers::*;
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

fn compiler_query_registry() -> nia_query::QueryRegistry {
    let mut registry = nia_query::QueryRegistry::new();
    macro_rules! register {
        ($($key:ty),+ $(,)?) => {
            $(registry.register::<CompilerContext, $key>();)+
        };
    }
    register!(
        AbiCheckQuery,
        ActiveModuleItemTreeInputQuery,
        ActiveModuleItemTreeQuery,
        BodyActivationWorklistQuery,
        BackendFinalizationTaskContextQuery,
        BackendItemPlanQuery,
        BackendLoweringInputsQuery,
        BackendModuleFinalizationQuery,
        BackendModuleItemPlanQuery,
        BackendModuleFunctionInstancePlanQuery,
        BackendModuleSourceItemPlanQuery,
        BackendLoweringQuery,
        BodyCheckQuery,
        CheckedModuleIdsQuery,
        CheckedModuleQuery,
        CheckedProgramQuery,
        CodegenPreparationQuery,
        CodegenProgramQuery,
        CompilerOptimizationQuery,
        CompilerRuntimeQuery,
        CompilerTargetQuery,
        ConstArrayLengthsQuery,
        ConstEnumValuesQuery,
        ConstModuleQuery,
        ConstQuery,
        ConstTypedFactsQuery,
        ConstValuesQuery,
        DeclarationActiveModuleItemTreeInputQuery,
        DeclarationActiveModuleItemTreeQuery,
        DeclarationModuleItemTreeInputQuery,
        DeclarationModuleItemTreeQuery,
        DeclarationTypeLoweringQuery,
        DeclarationTypeResolutionQuery,
        EntryCheckedProgramQuery,
        ExecutableCheckedModuleFactsQuery,
        ExecutableCheckedModulesQuery,
        ExecutableFactEpochQuery,
        ExecutableFunctionBodyQuery,
        ExecutableProviderDemandsQuery,
        ExecutableRootModulesQuery,
        ExecutableStaticInitQuery,
        ExecutableValueRefEdgesQuery,
        ExecutableValueRefItemIndexQuery,
        ExecutableValueRefItemQuery,
        ExtensionMethodByIdQuery,
        ExtensionMethodIndexQuery,
        ExtensionMethodsNamedQuery,
        ExtensionProviderDiscoveryIndexQuery,
        ExtensionProviderModuleEligibilityQuery,
        ExtensionProviderModuleFactsQuery,
        ExtensionProviderModuleIdsQuery,
        ExtensionProviderNominalCandidateModulesQuery,
        ExtensionProviderNominalModuleFactsQuery,
        ExtensionProviderNominalModulesForTargetsQuery,
        ExtensionProviderSummaryQuery,
        ExtensionProviderValidationFactsQuery,
        ExtensionSignatureModuleInputQuery,
        ExtensionTraitImplsForTraitQuery,
        ExtensionTraitSignatureIndexQuery,
        ExtensionTraitSolvingModuleFactsQuery,
        FlowCheckQuery,
        FrontendProgramSourcesQuery,
        FullActiveModuleItemTreeInputQuery,
        FullActiveModuleItemTreeQuery,
        FullModuleDefsQuery,
        FullModuleItemTreeInputQuery,
        FullModuleItemTreeQuery,
        ItemSignaturesQuery,
        LayoutTypeNormalizationQuery,
        LayoutsQuery,
        LoadedModulesQuery,
        LocalResolutionQuery,
        LoweredFunctionBodyQuery,
        ModuleAbiSignatureFactsQuery,
        ModuleDefsQuery,
        ModuleGraphChildQuery,
        ModuleGraphEntryQuery,
        ModuleGraphParentQuery,
        ModuleGraphPathQuery,
        ModuleGraphQuery,
        ModuleItemTreeInputQuery,
        ModuleItemTreeQuery,
        ModuleOriginsQuery,
        ModulePackageRootQuery,
        ModuleParseErrorsQuery,
        ModulePathQuery,
        ModuleProgramSignatureFactsQuery,
        ModulePublicSurfaceQuery,
        PublicSurfaceModuleFactsQuery,
        ModuleSourceVersionQuery,
        ModuleUsingScopeQuery,
        MonomorphizationQuery,
        ParseOkModuleIdsQuery,
        ProgramAbiSignaturesQuery,
        ProgramLoadDiagnosticsQuery,
        ProgramSignatureModuleEligibilityQuery,
        ProgramSignatureModuleIdsQuery,
        ProgramTraitMethodIndexQuery,
        ProgramTypeAliasSignatureQuery,
        ProviderFactRevisionQuery,
        ProviderFactWorklistQuery,
        PublicSurfaceModuleQuery,
        PublicSurfaceTypeQuery,
        PublicSurfaceValueQuery,
        PublicSurfacesQuery,
        PublicUsingScopesQuery,
        SemanticModuleIdsQuery,
        SemanticUseTableQuery,
        SignatureConstItemSignaturesQuery,
        SignatureConstItemTreeQuery,
        SignatureConstModuleQuery,
        SignatureConstTypeLoweringQuery,
        SignatureConstTypeNormalizationQuery,
        SignatureConstTypeResolutionQuery,
        SignatureItemSignaturesQuery,
        SignatureItemTreeQuery,
        SignatureLayoutsQuery,
        SignatureTypeLoweringQuery,
        SignatureTypeNormalizationQuery,
        SignatureTypeResolutionQuery,
        StaticCheckQuery,
        TypeExposureIndexQuery,
        TypeLoweringQuery,
        TypeNormalizationQuery,
        TypeResolutionQuery,
        UsingScopeModuleQuery,
        UsingScopeTypeQuery,
        UsingScopeUnresolvedQuery,
        UsingScopeValueQuery,
        ValueResolutionQuery,
        VisibleExtensionsQuery,
        VisibleTraitImplsQuery,
    );
    registry
}

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

struct CompilerContext {
    inputs: Arc<RwLock<CompilerInputs>>,
    loader_facts: Arc<dyn crate::LoaderFactProvider>,
    providers: CompilerQueryProviders,
    executable_fact_session: Arc<std::sync::Mutex<ExecutableFactSession>>,
    executable_fact_scheduler: std::sync::Mutex<()>,
    type_store: Arc<nia_ty::TypeStore>,
    diagnostic_store: nia_diagnostic::DiagnosticStore,
    node_store: nia_node_id::NodeStore,
    signature_cache: Option<Arc<crate::signature_cache::PersistentSignatureCache>>,
    verify_frontend_cache: bool,
    provider_demand_rounds: std::sync::atomic::AtomicU64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FrontendProgramSource {
    module: StableModuleKey,
    version: SourceVersion,
    len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FrontendProgramSources {
    fingerprint: crate::FrontendProgramSourceFingerprint,
    by_module: HashMap<ModuleId, FrontendProgramSource>,
    module_by_path: HashMap<String, ModuleId>,
    path_by_module: HashMap<ModuleId, String>,
}

#[derive(Debug, Clone)]
struct CheckCertificateContext {
    namespace: crate::FrontendCacheNamespace,
    entry: StableModuleKey,
    input: FrontendCheckInputFingerprint,
    scope: FrontendCheckScope,
    source_lengths: BTreeMap<String, usize>,
}

impl CheckCertificateContext {
    fn key(&self) -> FrontendCheckCertificateCacheKey {
        FrontendCheckCertificateCacheKey::new(self.namespace, &self.entry, self.input, self.scope)
    }

    fn identity(&self) -> crate::signature_cache::CheckCertificateIdentity<'_> {
        crate::signature_cache::CheckCertificateIdentity {
            key: self.key(),
            namespace: self.namespace,
            entry: &self.entry,
            input: self.input,
            scope: self.scope,
            source_lengths: &self.source_lengths,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct StableModuleSequence {
    keys: Vec<StableModuleKey>,
}

impl StableModuleSequence {
    fn from_source_identities(source_identities: impl IntoIterator<Item = SourceIdentity>) -> Self {
        Self {
            keys: source_identities
                .into_iter()
                .map(StableModuleKey::from_source_identity)
                .collect(),
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

#[derive(Debug, Clone)]
struct CompilerInputs {
    optimization: OptimizationPolicy,
    timings: TimingMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExecutableFactEpoch {
    entry_module: ModuleId,
    runtime_root_modules: Vec<ModuleId>,
    modules: Vec<(ModuleId, SourceVersion)>,
    target: TargetConfig,
    runtime: crate::RuntimeModel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BodyActivationWorklist {
    modules: Arc<HashMap<StableModuleKey, ModuleId>>,
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

impl CompilerInputs {
    fn new(request: CompileRequest) -> Self {
        Self {
            optimization: request.optimization.policy(),
            timings: request.timings,
        }
    }
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
    use nia_source::{SourceId, SourceRevision};
    use nia_symbol::{SymbolId, stable_hash};
    use std::ops::Deref;

    trait QueryDbTestExt<C> {
        fn expect_get<K>(&self, key: K) -> Arc<K::Value>
        where
            K: QueryKey<C>;
    }

    impl<C> QueryDbTestExt<C> for QueryDb<C> {
        fn expect_get<K>(&self, key: K) -> Arc<K::Value>
        where
            K: QueryKey<C>,
        {
            self.get(key).expect("test query must succeed")
        }
    }

    thread_local! {
        static TEST_SYMBOLS: nia_symbol_table::SymbolTable = nia_symbol_table::SymbolTable::new();
    }

    fn test_symbols() -> nia_symbol_table::SymbolTable {
        TEST_SYMBOLS.with(Clone::clone)
    }

    fn sym(text: &str) -> SymbolId {
        test_symbols()
            .intern(text)
            .unwrap_or_else(|err| panic!("test symbol collision: {err}"));
        SymbolId::from_stable_hash(stable_hash(text))
    }

    struct TestLoaderContext {
        program: RwLock<LoadedProgram>,
        provider_facts: RwLock<crate::ProviderFactSnapshot>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct TestLoadedProgramQuery;

    impl QueryKey<TestLoaderContext> for TestLoadedProgramQuery {
        type Value = LoadedProgram;

        const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

        fn name() -> &'static str {
            "test_loaded_program"
        }

        fn execute_result(&self, db: &QueryDb<TestLoaderContext>) -> QueryResult<Self::Value> {
            Ok(db
                .context()
                .program
                .read()
                .expect("test loader program lock poisoned")
                .clone())
        }

        fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
            old == new
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct TestProviderFactsQuery;

    impl QueryKey<TestLoaderContext> for TestProviderFactsQuery {
        type Value = crate::ProviderFactSnapshot;

        const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

        fn name() -> &'static str {
            "test_provider_facts"
        }

        fn execute_result(&self, db: &QueryDb<TestLoaderContext>) -> QueryResult<Self::Value> {
            Ok(db
                .context()
                .provider_facts
                .read()
                .expect("test provider facts lock poisoned")
                .clone())
        }

        fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
            old == new
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    enum TestLoaderFactKey {
        Graph,
        LoadedModuleSourceIdentities,
        ModulePath(ModuleId),
        ModuleSourceVersion(ModuleId),
        ModuleProviderSummary(ModuleId),
        ModuleOrigins(ModuleId),
        ModuleParseErrors(ModuleId),
        ModuleItemTree(ModuleId),
        ActiveModuleItemTree(ModuleId, ActiveModuleItemTreeFactKind),
        LoadDiagnostics,
        Target,
        Runtime,
    }

    #[derive(Debug, Clone, PartialEq)]
    enum TestLoaderFactValue {
        Graph(ModuleGraphSnapshot),
        LoadedModuleSourceIdentities(Vec<SourceIdentity>),
        ModulePath(Option<SourcePath>),
        ModuleSourceVersion(Option<SourceVersion>),
        ModuleProviderSummary(Option<nia_provider_summary::ProviderSummary>),
        ModuleOrigins(Option<NodeOriginTable>),
        ModuleParseErrors(Option<Vec<ParseError>>),
        ModuleItemTree(Option<ModuleItemTree>),
        ActiveModuleItemTree(Option<ActiveModuleItemTree>),
        LoadDiagnostics(Vec<ProgramDiagnostic>),
        Target(TargetConfig),
        Runtime(RuntimeModel),
    }

    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    struct TestLoaderFactQuery(TestLoaderFactKey);

    impl QueryKey<TestLoaderContext> for TestLoaderFactQuery {
        type Value = TestLoaderFactValue;

        const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

        fn name() -> &'static str {
            "test_loader_fact"
        }

        fn execute_result(&self, db: &QueryDb<TestLoaderContext>) -> QueryResult<Self::Value> {
            let program = db.get(TestLoadedProgramQuery)?;
            let module = |module_id| program.modules.iter().find(|module| module.id == module_id);
            Ok(match self.0 {
                TestLoaderFactKey::Graph => Self::Value::Graph(program.graph.clone()),
                TestLoaderFactKey::LoadedModuleSourceIdentities => {
                    Self::Value::LoadedModuleSourceIdentities(
                        program
                            .modules
                            .iter()
                            .map(|module| module.source_identity.clone())
                            .collect(),
                    )
                }
                TestLoaderFactKey::ModulePath(module_id) => {
                    Self::Value::ModulePath(module(module_id).map(|module| module.path.clone()))
                }
                TestLoaderFactKey::ModuleSourceVersion(module_id) => {
                    Self::Value::ModuleSourceVersion(
                        module(module_id).map(|module| module.source_version),
                    )
                }
                TestLoaderFactKey::ModuleProviderSummary(module_id) => {
                    Self::Value::ModuleProviderSummary(
                        module(module_id).map(|module| module.provider_summary.clone()),
                    )
                }
                TestLoaderFactKey::ModuleOrigins(module_id) => Self::Value::ModuleOrigins(
                    module(module_id).map(|module| module.origins.clone()),
                ),
                TestLoaderFactKey::ModuleParseErrors(module_id) => Self::Value::ModuleParseErrors(
                    module(module_id).map(|module| module.parse_errors.clone()),
                ),
                TestLoaderFactKey::ModuleItemTree(module_id) => Self::Value::ModuleItemTree(
                    module(module_id).map(|module| module.item_tree.clone()),
                ),
                TestLoaderFactKey::ActiveModuleItemTree(module_id, kind) => {
                    let tree = module(module_id).map(|module| match kind {
                        ActiveModuleItemTreeFactKind::Signature(set) => {
                            module.active_item_tree.signature_items(set)
                        }
                        ActiveModuleItemTreeFactKind::ConstSignature => {
                            module.active_item_tree.const_signature_items()
                        }
                        ActiveModuleItemTreeFactKind::Full => module.active_item_tree.clone(),
                    });
                    Self::Value::ActiveModuleItemTree(tree)
                }
                TestLoaderFactKey::LoadDiagnostics => {
                    Self::Value::LoadDiagnostics(program.diagnostics.clone())
                }
                TestLoaderFactKey::Target => Self::Value::Target(program.target.clone()),
                TestLoaderFactKey::Runtime => Self::Value::Runtime(program.runtime),
            })
        }

        fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
            old == new
        }
    }

    #[derive(Clone)]
    struct TestLoaderFacts {
        db: QueryDb<TestLoaderContext>,
    }

    impl TestLoaderFacts {
        fn new(program: LoadedProgram, provider_facts: crate::ProviderFactSnapshot) -> Self {
            let mut registry = nia_query::QueryRegistry::new();
            registry.register::<TestLoaderContext, TestLoadedProgramQuery>();
            registry.register::<TestLoaderContext, TestProviderFactsQuery>();
            registry.register::<TestLoaderContext, TestLoaderFactQuery>();
            Self {
                db: QueryDb::new_registered(
                    TestLoaderContext {
                        program: RwLock::new(program),
                        provider_facts: RwLock::new(provider_facts),
                    },
                    registry,
                ),
            }
        }

        fn program(&self) -> Arc<LoadedProgram> {
            self.db.expect_get(TestLoadedProgramQuery)
        }

        fn fact(&self, key: TestLoaderFactKey) -> Arc<TestLoaderFactValue> {
            self.db.expect_get(TestLoaderFactQuery(key))
        }

        fn replace_program(&self, program: LoadedProgram) -> nia_query::QueryInvalidation {
            let mut current = self
                .db
                .context()
                .program
                .write()
                .expect("test loader program lock poisoned");
            if *current == program {
                return nia_query::QueryInvalidation::default();
            }
            *current = program;
            drop(current);
            self.db.invalidate(TestLoadedProgramQuery)
        }

        fn replace_provider_facts(
            &self,
            provider_facts: crate::ProviderFactSnapshot,
        ) -> nia_query::QueryInvalidation {
            let mut current = self
                .db
                .context()
                .provider_facts
                .write()
                .expect("test provider facts lock poisoned");
            if *current == provider_facts {
                return nia_query::QueryInvalidation::default();
            }
            *current = provider_facts;
            drop(current);
            self.db.invalidate(TestProviderFactsQuery)
        }
    }

    impl crate::LoaderFactProvider for TestLoaderFacts {
        fn query_session(&self) -> Option<nia_query::QuerySession> {
            Some(self.db.session())
        }

        fn provider_facts(&self) -> QueryResult<crate::ProviderFactSnapshot> {
            Ok(self.db.get(TestProviderFactsQuery)?.as_ref().clone())
        }

        fn update_provider_demands(
            &self,
            _demands: Vec<crate::ProviderDemand>,
        ) -> QueryResult<crate::ProviderGraphUpdate> {
            Ok(crate::ProviderGraphUpdate::Stable)
        }

        fn node_store(&self) -> nia_node_id::NodeStore {
            self.program()
                .modules
                .first()
                .map(|module| module.origins.node_store().clone())
                .unwrap_or_default()
        }

        fn module_graph(&self) -> QueryResult<ModuleGraphSnapshot> {
            let fact = self.fact(TestLoaderFactKey::Graph);
            let TestLoaderFactValue::Graph(graph) = fact.as_ref() else {
                unreachable!()
            };
            Ok(graph.clone())
        }

        fn loaded_module_source_identities(&self) -> QueryResult<Vec<SourceIdentity>> {
            let fact = self.fact(TestLoaderFactKey::LoadedModuleSourceIdentities);
            let TestLoaderFactValue::LoadedModuleSourceIdentities(identities) = fact.as_ref()
            else {
                unreachable!()
            };
            Ok(identities.clone())
        }

        fn module_path(&self, module_id: ModuleId) -> QueryResult<Option<SourcePath>> {
            let fact = self.fact(TestLoaderFactKey::ModulePath(module_id));
            let TestLoaderFactValue::ModulePath(path) = fact.as_ref() else {
                unreachable!()
            };
            Ok(path.clone())
        }

        fn module_source_version(&self, module_id: ModuleId) -> QueryResult<Option<SourceVersion>> {
            let fact = self.fact(TestLoaderFactKey::ModuleSourceVersion(module_id));
            let TestLoaderFactValue::ModuleSourceVersion(version) = fact.as_ref() else {
                unreachable!()
            };
            Ok(*version)
        }

        fn module_source_fingerprint(
            &self,
            _module_id: ModuleId,
        ) -> QueryResult<Option<(crate::SourceContentFingerprint, usize)>> {
            Ok(None)
        }

        fn module_provider_summary(
            &self,
            module_id: ModuleId,
        ) -> QueryResult<Option<nia_provider_summary::ProviderSummary>> {
            let fact = self.fact(TestLoaderFactKey::ModuleProviderSummary(module_id));
            let TestLoaderFactValue::ModuleProviderSummary(summary) = fact.as_ref() else {
                unreachable!()
            };
            Ok(summary.clone())
        }

        fn module_origins(&self, module_id: ModuleId) -> QueryResult<Option<NodeOriginTable>> {
            let fact = self.fact(TestLoaderFactKey::ModuleOrigins(module_id));
            let TestLoaderFactValue::ModuleOrigins(origins) = fact.as_ref() else {
                unreachable!()
            };
            Ok(origins.clone())
        }

        fn module_parse_errors(&self, module_id: ModuleId) -> QueryResult<Option<Vec<ParseError>>> {
            let fact = self.fact(TestLoaderFactKey::ModuleParseErrors(module_id));
            let TestLoaderFactValue::ModuleParseErrors(errors) = fact.as_ref() else {
                unreachable!()
            };
            Ok(errors.clone())
        }

        fn module_item_tree(&self, module_id: ModuleId) -> QueryResult<Option<ModuleItemTree>> {
            let fact = self.fact(TestLoaderFactKey::ModuleItemTree(module_id));
            let TestLoaderFactValue::ModuleItemTree(tree) = fact.as_ref() else {
                unreachable!()
            };
            Ok(tree.clone())
        }

        fn active_module_item_tree(
            &self,
            module_id: ModuleId,
            kind: ActiveModuleItemTreeFactKind,
        ) -> QueryResult<Option<ActiveModuleItemTree>> {
            let fact = self.fact(TestLoaderFactKey::ActiveModuleItemTree(module_id, kind));
            let TestLoaderFactValue::ActiveModuleItemTree(tree) = fact.as_ref() else {
                unreachable!()
            };
            Ok(tree.clone())
        }

        fn load_diagnostics(&self) -> QueryResult<Vec<ProgramDiagnostic>> {
            let fact = self.fact(TestLoaderFactKey::LoadDiagnostics);
            let TestLoaderFactValue::LoadDiagnostics(diagnostics) = fact.as_ref() else {
                unreachable!()
            };
            Ok(diagnostics.clone())
        }

        fn symbols(&self) -> nia_symbol_table::SymbolTable {
            self.program().symbols.clone()
        }

        fn target(&self) -> TargetConfig {
            let fact = self.fact(TestLoaderFactKey::Target);
            let TestLoaderFactValue::Target(target) = fact.as_ref() else {
                unreachable!()
            };
            target.clone()
        }

        fn runtime(&self) -> RuntimeModel {
            let fact = self.fact(TestLoaderFactKey::Runtime);
            let TestLoaderFactValue::Runtime(runtime) = fact.as_ref() else {
                unreachable!()
            };
            *runtime
        }
    }

    #[derive(Clone)]
    struct CompilerDatabase {
        compiler: super::CompilerDatabase,
        loader: TestLoaderFacts,
    }

    impl CompilerDatabase {
        fn new(request: CompileRequest) -> Self {
            let provider_facts = request
                .loader_facts
                .provider_facts()
                .expect("test provider facts");
            let program = materialize_loader_facts(request.loader_facts.as_ref());
            let loader = TestLoaderFacts::new(program, provider_facts);
            let request = request.with_loader_facts(loader.clone());
            let compiler = super::CompilerDatabase::new(request);
            Self { compiler, loader }
        }

        fn update(&self, request: CompileRequest) -> CompilerInvalidation {
            let mut program = materialize_loader_facts(request.loader_facts.as_ref());
            program.provider_fact_revision =
                crate::LoaderFactProvider::provider_facts(&self.loader)
                    .expect("test provider facts")
                    .revision();
            self.loader.replace_program(program);
            let request = request.with_loader_facts(self.loader.clone());
            self.compiler.update(request).expect("test compiler update")
        }

        fn check_program(&self) -> CheckedProgram {
            self.compiler.check_program().expect("test compiler check")
        }

        fn analyze_program(&self) -> CheckedProgramAnalysis {
            self.compiler
                .analyze_program()
                .expect("test compiler analysis")
        }

        fn codegen_program(&self) -> CodegenProgram {
            self.compiler
                .codegen_program()
                .expect("test codegen program")
        }

        fn codegen_preparation(&self) -> CodegenPreparation {
            self.compiler
                .codegen_preparation()
                .expect("test codegen preparation")
        }

        fn replace_provider_facts(
            &self,
            provider_facts: crate::ProviderFactSnapshot,
        ) -> nia_query::QueryInvalidation {
            self.loader.replace_provider_facts(provider_facts)
        }
    }

    impl Deref for CompilerDatabase {
        type Target = super::CompilerDatabase;

        fn deref(&self) -> &Self::Target {
            &self.compiler
        }
    }

    fn materialize_loader_facts(facts: &dyn crate::LoaderFactProvider) -> LoadedProgram {
        let graph = facts.module_graph().expect("test module graph");
        let modules = facts
            .loaded_module_source_identities()
            .expect("test loaded module identities")
            .into_iter()
            .map(|identity| {
                let module_id = graph
                    .modules()
                    .find_map(|module| {
                        facts
                            .module_path(module.id)
                            .expect("test module path query")
                            .is_some_and(|path| path.identity() == identity)
                            .then_some(module.id)
                    })
                    .expect("test loaded module identity must resolve in graph");
                LoadedModule {
                    id: module_id,
                    path: facts
                        .module_path(module_id)
                        .expect("test module path query")
                        .expect("test module path"),
                    source_identity: facts
                        .module_path(module_id)
                        .expect("test module path query")
                        .expect("test module path")
                        .identity(),
                    source_version: facts
                        .module_source_version(module_id)
                        .expect("test module source version query")
                        .expect("test module source version"),
                    item_tree: facts
                        .module_item_tree(module_id)
                        .expect("test module item tree query")
                        .expect("test module item tree"),
                    active_item_tree: facts
                        .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::Full)
                        .expect("test active module item tree query")
                        .expect("test active module item tree"),
                    provider_summary: facts
                        .module_provider_summary(module_id)
                        .expect("test module provider summary query")
                        .expect("test module provider summary"),
                    origins: facts
                        .module_origins(module_id)
                        .expect("test module origins query")
                        .expect("test module origins"),
                    parse_errors: facts
                        .module_parse_errors(module_id)
                        .expect("test module parse errors query")
                        .expect("test module parse errors"),
                }
            })
            .collect();
        LoadedProgram {
            graph,
            provider_fact_revision: facts
                .provider_facts()
                .expect("test provider facts")
                .revision(),
            symbols: facts.symbols(),
            target: facts.target(),
            runtime: facts.runtime(),
            modules,
            diagnostics: facts.load_diagnostics().expect("test load diagnostics"),
        }
    }

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

    fn intern_child(
        graph: &mut ModuleGraph,
        parent: ModuleId,
        child_name: &str,
        visibility: nia_ids::Visibility,
    ) -> ModuleId {
        let child = sym(child_name);
        graph
            .intern_declared_child(parent, &child, visibility, Span::default())
            .expect("intern child module")
    }

    fn intern_shallow_child(
        graph: &mut ModuleGraph,
        parent: ModuleId,
        child_name: &str,
        visibility: nia_ids::Visibility,
    ) -> ModuleId {
        let child = sym(child_name);
        graph
            .intern_declared_child_with_processing(
                parent,
                &child,
                visibility,
                Span::default(),
                false,
                false,
            )
            .expect("intern shallow child module")
    }

    struct LoadedProgramFixture {
        graph: ModuleGraph,
        modules: Vec<LoadedModule>,
    }

    impl LoadedProgramFixture {
        fn new(entry_path: &str, source: &str) -> Self {
            let graph = ModuleGraph::with_symbol_text(
                SourcePath::new(entry_path),
                Arc::new(test_symbols()),
            );
            let entry_id = graph.entry();
            Self {
                graph,
                modules: vec![loaded_module(entry_id, entry_path, source)],
            }
        }

        fn entry_id(&self) -> ModuleId {
            self.graph.entry()
        }

        fn add_child(
            &mut self,
            parent: ModuleId,
            child_name: &str,
            path: &str,
            source: &str,
        ) -> ModuleId {
            self.add_child_with_visibility(
                parent,
                child_name,
                nia_ids::Visibility::Public,
                path,
                source,
            )
        }

        fn add_child_with_visibility(
            &mut self,
            parent: ModuleId,
            child_name: &str,
            visibility: nia_ids::Visibility,
            path: &str,
            source: &str,
        ) -> ModuleId {
            let module_id = intern_child(&mut self.graph, parent, child_name, visibility);
            self.modules.push(loaded_module(module_id, path, source));
            module_id
        }

        fn add_shallow_child(
            &mut self,
            parent: ModuleId,
            child_name: &str,
            path: &str,
            source: &str,
        ) -> ModuleId {
            let module_id = intern_shallow_child(
                &mut self.graph,
                parent,
                child_name,
                nia_ids::Visibility::Public,
            );
            self.modules.push(loaded_module(module_id, path, source));
            module_id
        }

        fn update_module_source(
            &mut self,
            module_id: ModuleId,
            source: &str,
            revision: SourceRevision,
        ) {
            let module = self
                .modules
                .iter_mut()
                .find(|module| module.id == module_id)
                .expect("fixture module");
            *module =
                loaded_module_with_revision(module_id, module.path.as_str(), source, revision);
        }

        fn update_module_path(&mut self, module_id: ModuleId, path: &str) {
            let module = self
                .modules
                .iter_mut()
                .find(|module| module.id == module_id)
                .expect("fixture module");
            module.path = SourcePath::new(path);
            module.source_identity = module.path.identity();
        }

        fn program(&self) -> LoadedProgram {
            LoadedProgram {
                graph: self.graph.clone().into(),
                provider_fact_revision: crate::ProviderFactRevision::default(),
                symbols: test_symbols(),
                target: TargetConfig::host(),
                runtime: RuntimeModel::Bare,
                modules: self.modules.clone(),
                diagnostics: Vec::new(),
            }
        }

        fn database(&self) -> CompilerDatabase {
            CompilerDatabase::new(CompileRequest::new(self.program()))
        }
    }

    fn loaded_module(id: ModuleId, path: &str, source: &str) -> LoadedModule {
        loaded_module_with_revision(id, path, source, SourceRevision::INITIAL)
    }

    fn loaded_module_with_revision(
        id: ModuleId,
        path: &str,
        source: &str,
        revision: SourceRevision,
    ) -> LoadedModule {
        loaded_module_with_source_version(
            id,
            path,
            source,
            SourceVersion {
                id: SourceId(id.local_index()),
                revision,
            },
        )
    }

    fn loaded_module_with_source_version(
        id: ModuleId,
        path: &str,
        source: &str,
        source_version: SourceVersion,
    ) -> LoadedModule {
        let syntax = nia_syntax::parse_source(source, Some(source_version));
        let (module, parse_errors, origins) =
            nia_parser::parse_module_syntax_with_origins_and_symbols(&syntax, test_symbols());
        assert!(parse_errors.is_empty(), "{parse_errors:?}");
        let item_tree = ModuleItemTree::from_module(&module);
        let active_item_tree =
            ActiveModuleItemTree::new(item_tree.items.clone(), Default::default());
        let provider_summary =
            nia_provider_summary::ProviderSummary::from_active_item_tree(&active_item_tree);
        LoadedModule {
            id,
            path: SourcePath::new(path),
            source_identity: SourcePath::new(path).identity(),
            source_version,
            item_tree: item_tree.clone(),
            active_item_tree,
            provider_summary,
            parse_errors,
            origins,
        }
    }

    fn query_db(loaded: LoadedProgram) -> QueryDb<CompilerContext> {
        let loader_facts: Arc<dyn crate::LoaderFactProvider> = Arc::new(loaded.clone());
        let node_store = loader_facts.node_store();
        let inputs = Arc::new(RwLock::new(CompilerInputs::new(CompileRequest::new(
            loaded,
        ))));
        QueryDb::new_registered(
            CompilerContext {
                inputs,
                loader_facts,
                providers: CompilerQueryProviders::default(),
                executable_fact_session: Arc::new(std::sync::Mutex::new(
                    ExecutableFactSession::default(),
                )),
                executable_fact_scheduler: std::sync::Mutex::new(()),
                type_store: Arc::new(nia_ty::TypeStore::new()),
                diagnostic_store: nia_diagnostic::DiagnosticStore::new(),
                node_store,
                signature_cache: None,
                verify_frontend_cache: false,
                provider_demand_rounds: std::sync::atomic::AtomicU64::new(0),
            },
            compiler_query_registry(),
        )
    }

    struct FingerprintedLoadedProgram {
        program: LoadedProgram,
        sources: HashMap<ModuleId, (crate::SourceContentFingerprint, usize)>,
    }

    impl crate::LoaderFactProvider for FingerprintedLoadedProgram {
        fn query_session(&self) -> Option<nia_query::QuerySession> {
            None
        }

        fn provider_facts(&self) -> QueryResult<crate::ProviderFactSnapshot> {
            self.program.provider_facts()
        }

        fn update_provider_demands(
            &self,
            demands: Vec<crate::ProviderDemand>,
        ) -> QueryResult<crate::ProviderGraphUpdate> {
            self.program.update_provider_demands(demands)
        }

        fn node_store(&self) -> nia_node_id::NodeStore {
            self.program.node_store()
        }

        fn module_graph(&self) -> QueryResult<nia_imports::ModuleGraphSnapshot> {
            self.program.module_graph()
        }

        fn loaded_module_source_identities(&self) -> QueryResult<Vec<SourceIdentity>> {
            self.program.loaded_module_source_identities()
        }

        fn module_path(&self, module_id: ModuleId) -> QueryResult<Option<SourcePath>> {
            self.program.module_path(module_id)
        }

        fn module_source_version(&self, module_id: ModuleId) -> QueryResult<Option<SourceVersion>> {
            self.program.module_source_version(module_id)
        }

        fn module_source_fingerprint(
            &self,
            module_id: ModuleId,
        ) -> QueryResult<Option<(crate::SourceContentFingerprint, usize)>> {
            Ok(self.sources.get(&module_id).copied())
        }

        fn module_provider_summary(
            &self,
            module_id: ModuleId,
        ) -> QueryResult<Option<nia_provider_summary::ProviderSummary>> {
            self.program.module_provider_summary(module_id)
        }

        fn module_origins(&self, module_id: ModuleId) -> QueryResult<Option<NodeOriginTable>> {
            self.program.module_origins(module_id)
        }

        fn module_parse_errors(&self, module_id: ModuleId) -> QueryResult<Option<Vec<ParseError>>> {
            self.program.module_parse_errors(module_id)
        }

        fn module_item_tree(&self, module_id: ModuleId) -> QueryResult<Option<ModuleItemTree>> {
            self.program.module_item_tree(module_id)
        }

        fn active_module_item_tree(
            &self,
            module_id: ModuleId,
            kind: ActiveModuleItemTreeFactKind,
        ) -> QueryResult<Option<ActiveModuleItemTree>> {
            self.program.active_module_item_tree(module_id, kind)
        }

        fn load_diagnostics(&self) -> QueryResult<Vec<ProgramDiagnostic>> {
            self.program.load_diagnostics()
        }

        fn symbols(&self) -> nia_symbol_table::SymbolTable {
            self.program.symbols()
        }

        fn target(&self) -> TargetConfig {
            self.program.target()
        }

        fn runtime(&self) -> RuntimeModel {
            self.program.runtime()
        }
    }

    fn query_db_with_frontend_cache(
        loaded: LoadedProgram,
        sources: HashMap<ModuleId, (crate::SourceContentFingerprint, usize)>,
        root: PathBuf,
        verify: bool,
    ) -> QueryDb<CompilerContext> {
        let loader_facts: Arc<dyn crate::LoaderFactProvider> =
            Arc::new(FingerprintedLoadedProgram {
                program: loaded.clone(),
                sources,
            });
        let node_store = loader_facts.node_store();
        let inputs = Arc::new(RwLock::new(CompilerInputs::new(
            CompileRequest::new(loaded)
                .with_frontend_cache_dir(Some(root.clone()))
                .with_frontend_cache_verification(verify),
        )));
        QueryDb::new_registered(
            CompilerContext {
                inputs,
                loader_facts,
                providers: CompilerQueryProviders::default(),
                executable_fact_session: Arc::new(std::sync::Mutex::new(
                    ExecutableFactSession::default(),
                )),
                executable_fact_scheduler: std::sync::Mutex::new(()),
                type_store: Arc::new(nia_ty::TypeStore::new()),
                diagnostic_store: nia_diagnostic::DiagnosticStore::new(),
                node_store,
                signature_cache: Some(Arc::new(
                    crate::signature_cache::PersistentSignatureCache::new(root),
                )),
                verify_frontend_cache: verify,
                provider_demand_rounds: std::sync::atomic::AtomicU64::new(0),
            },
            compiler_query_registry(),
        )
    }

    fn module_id_for_source_identity(
        db: &QueryDb<CompilerContext>,
        identity: &SourceIdentity,
    ) -> Option<ModuleId> {
        db.context()
            .loader_facts
            .module_graph()
            .expect("test module graph")
            .modules()
            .find_map(|module| {
                db.context()
                    .loader_facts
                    .module_path(module.id)
                    .expect("test module path query")
                    .is_some_and(|path| path.identity() == *identity)
                    .then_some(module.id)
            })
    }

    fn query_executions(trace: &QueryTrace, name: &'static str) -> usize {
        trace
            .queries
            .iter()
            .filter(|query| query.frame.name == name)
            .map(|query| query.stats.executions)
            .sum()
    }

    fn query_cache_hits(trace: &QueryTrace, name: &'static str) -> usize {
        trace
            .queries
            .iter()
            .filter(|query| query.frame.name == name)
            .map(|query| query.stats.cache_hits)
            .sum()
    }

    fn query_green_validations(trace: &QueryTrace, name: &'static str) -> usize {
        trace
            .queries
            .iter()
            .filter(|query| query.frame.name == name)
            .map(|query| query.stats.green_validations)
            .sum()
    }

    fn is_body_signature_query(name: &str) -> bool {
        matches!(name, "program_body_function_signatures")
    }

    fn trace_has_dependency(trace: &QueryTrace, from: &str, to: &str) -> bool {
        trace
            .dependencies
            .iter()
            .any(|dependency| dependency.from.name == from && dependency.to.name == to)
    }

    fn depends_on_body_signature_query(trace: &QueryTrace, from: &str) -> bool {
        trace.dependencies.iter().any(|dependency| {
            dependency.from.name == from && is_body_signature_query(dependency.to.name)
        })
    }

    fn assert_query_executions_unchanged(
        before: &QueryTrace,
        after: &QueryTrace,
        name: &'static str,
    ) {
        assert_eq!(
            query_executions(before, name),
            query_executions(after, name),
            "{name} should have been reused"
        );
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
    fn semantic_module_ids_exclude_shallow_facade_modules() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
pub module facade;

fn main() i32 {
    0
}
"#,
        );
        let entry_id = fixture.entry_id();
        let facade_id = fixture.add_shallow_child(
            entry_id,
            "facade",
            "facade.nia",
            r#"
pub fn expensive_or_invalid() i32 {
    missing_symbol
}
"#,
        );
        let db = query_db(fixture.program());

        assert_eq!(
            resolve_stable_module_sequence(&db, &db.expect_get(ParseOkModuleIdsQuery))
                .expect("parse-ok module sequence")
                .as_slice(),
            &[entry_id, facade_id]
        );
        assert_eq!(
            resolve_stable_module_sequence(&db, &db.expect_get(SemanticModuleIdsQuery))
                .expect("semantic module sequence")
                .as_slice(),
            &[entry_id]
        );

        assert_eq!(db.expect_get(CheckedModuleIdsQuery).as_slice(), &[entry_id]);
    }

    #[test]
    fn loader_facts_map_modules_by_source_identity() {
        let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let entry_id = fixture.entry_id();
        let package_id =
            fixture.add_child(entry_id, "pkg", "pkg/root.nia", "pub fn value() i32 { 1 }");
        let db = query_db(fixture.program());

        assert_eq!(
            module_id_for_source_identity(&db, &SourcePath::new("pkg/root.nia").identity()),
            Some(package_id)
        );
    }

    #[test]
    fn loaded_module_reorder_invalidates_list_without_field_changes() {
        let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let entry_id = fixture.entry_id();
        let package_id =
            fixture.add_child(entry_id, "pkg", "pkg/root.nia", "pub fn value() i32 { 1 }");
        let database = fixture.database();
        let first = database.db.expect_get(LoadedModulesQuery);
        let mut reordered = fixture.program();
        reordered.modules.reverse();
        database.update(CompileRequest::new(reordered));
        let latest = database.db.expect_get(LoadedModulesQuery);
        assert!(!Arc::ptr_eq(&first, &latest));
        assert_eq!(
            resolve_stable_module_sequence(&database.db, &latest)
                .expect("reordered module sequence"),
            vec![package_id, entry_id]
        );
    }

    #[test]
    fn additive_module_growth_refreshes_query_derived_executable_epoch() {
        let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let entry_id = fixture.entry_id();
        let database = fixture.database();
        let first = database.db.expect_get(ExecutableFactEpochQuery);
        fixture.add_child(
            entry_id,
            "provider",
            "main/provider.nia",
            "pub fn value() i32 { 1 }",
        );
        database.update(CompileRequest::new(fixture.program()));
        let latest = database.db.expect_get(ExecutableFactEpochQuery);

        assert_ne!(first.as_ref(), latest.as_ref());
        assert_eq!(first.modules.len() + 1, latest.modules.len());
        assert_eq!(first.modules[0], latest.modules[0]);
    }

    #[test]
    fn stable_graph_entry_remaps_after_module_graph_owner_replacement() {
        let old_fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let new_fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let old_entry = old_fixture.entry_id();
        let new_entry = new_fixture.entry_id();
        assert_ne!(old_entry, new_entry);
        let database = old_fixture.database();
        let first = database.db.expect_get(ModuleGraphEntryQuery);
        let first_loaded = database.db.expect_get(LoadedModulesQuery);

        database.update(CompileRequest::new(new_fixture.program()));

        let latest = database.db.expect_get(ModuleGraphEntryQuery);
        let latest_loaded = database.db.expect_get(LoadedModulesQuery);
        assert!(!Arc::ptr_eq(&first, &latest));
        assert_eq!(first.as_ref(), latest.as_ref());
        assert!(Arc::ptr_eq(&first_loaded, &latest_loaded));
        assert_eq!(
            resolve_stable_module_sequence(&database.db, &latest_loaded)
                .expect("remapped module sequence"),
            vec![new_entry]
        );
        assert_eq!(
            QueryModuleGraphLookup::new(&database.db)
                .expect("module graph lookup should load")
                .entry_module(),
            new_entry
        );
    }

    #[test]
    fn stable_graph_relations_remap_fork_local_module_handles() {
        let base = LoadedProgramFixture::new(
            "main.nia",
            "pub module child; using self::child; fn main() i32 { 0 }",
        );
        let entry = base.entry_id();
        let mut old_fixture = LoadedProgramFixture {
            graph: base.graph.clone(),
            modules: base.modules.clone(),
        };
        let mut new_fixture = LoadedProgramFixture {
            graph: base.graph.clone(),
            modules: base.modules,
        };
        let child_name = sym("child");
        let package = sym("pkg");
        let old_child =
            old_fixture.add_child(entry, "child", "main/child.nia", "pub fn value() i32 { 1 }");
        let new_child =
            new_fixture.add_child(entry, "child", "main/child.nia", "pub fn value() i32 { 1 }");
        let old_root = old_fixture
            .graph
            .intern_package_root(&package, SourcePath::new("pkg/root.nia"));
        old_fixture.modules.push(loaded_module(
            old_root,
            "pkg/root.nia",
            "pub fn root() i32 { 1 }",
        ));
        let new_root = new_fixture
            .graph
            .intern_package_root(&package, SourcePath::new("pkg/root.nia"));
        new_fixture.modules.push(loaded_module(
            new_root,
            "pkg/root.nia",
            "pub fn root() i32 { 1 }",
        ));
        assert_ne!(old_child, new_child);
        assert_ne!(old_root, new_root);
        let database = old_fixture.database();
        let first_child = database
            .db
            .expect_get(ModuleGraphChildQuery(entry, child_name));
        let first_root = database.db.expect_get(ModulePackageRootQuery(package));
        let first_public = database
            .db
            .expect_get(PublicSurfaceModuleQuery(entry, child_name));
        let first_using = database
            .db
            .expect_get(UsingScopeModuleQuery(entry, child_name));

        database.update(CompileRequest::new(new_fixture.program()));

        let latest_child = database
            .db
            .expect_get(ModuleGraphChildQuery(entry, child_name));
        let latest_root = database.db.expect_get(ModulePackageRootQuery(package));
        let latest_public = database
            .db
            .expect_get(PublicSurfaceModuleQuery(entry, child_name));
        let latest_using = database
            .db
            .expect_get(UsingScopeModuleQuery(entry, child_name));
        assert!(!Arc::ptr_eq(&first_child, &latest_child));
        assert!(!Arc::ptr_eq(&first_root, &latest_root));
        assert_eq!(first_child.as_ref(), latest_child.as_ref());
        assert_eq!(first_root.as_ref(), latest_root.as_ref());
        assert!(!Arc::ptr_eq(&first_public, &latest_public));
        assert!(!Arc::ptr_eq(&first_using, &latest_using));
        assert_eq!(first_public.as_ref(), latest_public.as_ref());
        assert_eq!(first_using.as_ref(), latest_using.as_ref());
        let lookup =
            QueryModuleGraphLookup::new(&database.db).expect("module graph lookup should load");
        assert_eq!(
            lookup.child_declaration(entry, &child_name),
            Some((new_child, nia_ids::Visibility::Public))
        );
        assert_eq!(lookup.package_root_module(&package), Some(new_root));
        assert_eq!(
            QueryPublicSurfaceLookup::new(&database.db).public_module(entry, &child_name),
            Some(new_child)
        );
        assert_eq!(
            QueryUsingScopeLookup::new(&database.db, entry).using_module(&child_name),
            Some(new_child)
        );
    }

    #[test]
    fn stable_source_identity_with_new_module_id_invalidates_old_key_and_recomputes_new_key() {
        let source = "pub struct S { value: i32 } fn main() i32 { 0 }";
        let old_fixture = LoadedProgramFixture::new("main.nia", source);
        let old_program = old_fixture.program();

        let mut new_fixture = LoadedProgramFixture::new("bootstrap.nia", "");
        let new_module_id = new_fixture
            .graph
            .intern_package_root(&sym("replacement"), SourcePath::new("main.nia"));
        new_fixture.graph.mark_process_used_paths(new_module_id);
        new_fixture.modules = vec![loaded_module(new_module_id, "main.nia", source)];
        let new_program = new_fixture.program();

        let database = CompilerDatabase::new(CompileRequest::new(old_program));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
        let first_loaded = database.db.expect_get(LoadedModulesQuery);

        database.update(CompileRequest::new(new_program));
        let latest_loaded = database.db.expect_get(LoadedModulesQuery);
        assert!(Arc::ptr_eq(&first_loaded, &latest_loaded));
        assert_eq!(
            resolve_stable_module_sequence(&database.db, &latest_loaded)
                .expect("updated module sequence"),
            vec![new_module_id]
        );

        let second = database.analyze_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        assert_eq!(second.modules[0].id, new_module_id);
    }

    #[test]
    fn tracked_loader_update_refreshes_changed_module_field_inputs() {
        let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let module_id = fixture.entry_id();
        let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
        let first_source_version = database.db.expect_get(ModuleSourceVersionQuery(module_id));

        fixture.update_module_source(module_id, "fn main() i32 { true }", SourceRevision(1));
        database.update(CompileRequest::new(fixture.program()));
        let latest_source_version = database.db.expect_get(ModuleSourceVersionQuery(module_id));
        assert!(!Arc::ptr_eq(&first_source_version, &latest_source_version));
        assert_eq!(latest_source_version.revision, SourceRevision(1));

        let second = database.check_program();
        assert!(!second.diagnostics.is_empty());
        assert!(
            database
                .query_trace()
                .dependencies
                .iter()
                .any(|dependency| {
                    dependency.from.name == "semantic_module_ids"
                        && dependency.to.name == "parse_ok_module_ids"
                })
        );
    }

    #[test]
    fn source_handle_replacement_cannot_reuse_old_source_version() {
        let source = "fn main() i32 { 0 }";
        let mut fixture = LoadedProgramFixture::new("main.nia", source);
        let module_id = fixture.entry_id();
        let database = fixture.database();
        let first = database.db.expect_get(ModuleSourceVersionQuery(module_id));
        let replacement = SourceVersion {
            id: SourceId(first.id.0 + 1),
            revision: first.revision,
        };
        fixture.modules[0] =
            loaded_module_with_source_version(module_id, "main.nia", source, replacement);

        database.update(CompileRequest::new(fixture.program()));
        let latest = database.db.expect_get(ModuleSourceVersionQuery(module_id));

        assert!(!Arc::ptr_eq(&first, &latest));
        assert_eq!(*latest, replacement);
    }

    #[test]
    fn timing_mode_update_does_not_invalidate_semantic_queries() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let loaded = fixture.program();
        let database = CompilerDatabase::new(CompileRequest::new(loaded.clone()));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
        let before_update = database.query_trace();

        let invalidation =
            database.update(CompileRequest::new(loaded).with_timings(crate::TimingMode::Summary));
        assert!(
            invalidation.invalidated.is_empty(),
            "{:?}",
            invalidation.invalidated
        );

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        let after_second_check = database.query_trace();

        assert_query_executions_unchanged(&before_update, &after_second_check, "checked_program");
        assert_query_executions_unchanged(
            &before_update,
            &after_second_check,
            "checked_module_ids",
        );
        assert_query_executions_unchanged(&before_update, &after_second_check, "checked_module");
    }

    #[test]
    fn provider_graph_growth_recomputes_query_derived_executable_roots() {
        let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let entry_id = fixture.entry_id();
        let provider_id = fixture.add_shallow_child(
            entry_id,
            "provider",
            "main/provider.nia",
            "pub fn value() i32 { 1 }",
        );
        let loaded = fixture.program();
        let database = CompilerDatabase::new(CompileRequest::new(loaded.clone()));
        assert_eq!(
            database.db.expect_get(ExecutableRootModulesQuery).as_ref(),
            &(entry_id, Vec::new())
        );
        let _ = database.db.expect_get(TypeResolutionQuery(entry_id));

        let mut grown = loaded;
        let mut graph = (*grown.graph).clone();
        assert!(graph.mark_semantic_selected(provider_id));
        grown.graph = graph.into();
        let before_update = database.query_trace();
        database.update(CompileRequest::new(grown));
        assert_eq!(
            database.db.expect_get(ExecutableRootModulesQuery).as_ref(),
            &(entry_id, Vec::new())
        );
        let after_update = database.query_trace();
        assert_query_executions_unchanged(&before_update, &after_update, "type_resolution");
        assert!(
            query_executions(&before_update, "executable_root_modules")
                < query_executions(&after_update, "executable_root_modules")
        );
    }

    #[test]
    fn additive_provider_graph_growth_reuses_existing_executable_facts() {
        let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let entry_id = fixture.entry_id();
        let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));

        let _ = database.executable_provider_demands();
        {
            let session = database
                .db
                .context()
                .executable_fact_session
                .lock()
                .expect("executable fact session lock poisoned");
            assert!(session.modules.contains_key(&entry_id));
            assert!(
                session
                    .caches
                    .body_resolution_inputs
                    .borrow()
                    .contains_key(&entry_id)
            );
        }

        fixture.add_child(
            entry_id,
            "provider",
            "main/provider.nia",
            "pub fn value() i32 { 1 }",
        );
        database.update(CompileRequest::new(fixture.program()));
        let session = database
            .db
            .context()
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned");
        assert!(session.modules.contains_key(&entry_id));
        assert!(
            session
                .caches
                .body_resolution_inputs
                .borrow()
                .contains_key(&entry_id)
        );
    }

    #[test]
    fn provider_changes_discard_affected_executable_fact_caches() {
        let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let entry_id = fixture.entry_id();
        let revision = crate::ProviderFactRevision::new_store();
        let mut program = fixture.program();
        program.provider_fact_revision = revision;
        let database = CompilerDatabase::new(CompileRequest::new(program));
        let _ = database.executable_provider_demands();
        let provider_changes = vec![crate::ProviderDemand {
            source_path: SourcePath::new("main.nia"),
            request: crate::ProviderRequest::Method {
                target_type_name: None,
                method_name: SymbolId::default(),
            },
        }];
        {
            let mut session = database
                .db
                .context()
                .executable_fact_session
                .lock()
                .expect("executable fact session lock poisoned");
            let state = session
                .modules
                .get_mut(&entry_id)
                .expect("entry executable facts");
            state
                .unowned_provider_demands
                .insert(provider_changes[0].clone());
            state.provider_demands.insert(provider_changes[0].clone());
        }

        fixture.add_child(
            entry_id,
            "provider",
            "main/provider.nia",
            "pub fn value() i32 { 1 }",
        );
        database.update(CompileRequest::new(fixture.program()));
        database.replace_provider_facts(crate::ProviderFactSnapshot::new(
            revision.next(),
            revision,
            provider_changes,
        ));

        {
            let session = database
                .db
                .context()
                .executable_fact_session
                .lock()
                .expect("executable fact session lock poisoned");
            assert!(session.modules.contains_key(&entry_id));
            assert_eq!(session.applied_provider_fact_revision, Some(revision));
        }
        let worklist = database.db.expect_get(ProviderFactWorklistQuery);
        let mut session = database
            .db
            .context()
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned");
        session.apply_provider_fact_worklist(&worklist, &database.db.context().type_store);
        assert!(!session.modules.contains_key(&entry_id));
        assert!(
            !session
                .caches
                .body_resolution_inputs
                .borrow()
                .contains_key(&entry_id)
        );
        assert_eq!(
            session.applied_provider_fact_revision,
            Some(revision.next())
        );
    }

    #[test]
    fn provider_fact_snapshot_deduplicates_demands() {
        let revision = crate::ProviderFactRevision::new_store();
        let demand = crate::ProviderDemand {
            source_path: SourcePath::new("main.nia"),
            request: crate::ProviderRequest::Method {
                target_type_name: None,
                method_name: SymbolId::default(),
            },
        };
        let facts = crate::ProviderFactSnapshot::new(revision, revision, [demand.clone(), demand]);
        assert_eq!(facts.demands().len(), 1);
    }

    #[test]
    fn check_certificate_input_covers_stable_graph_and_provider_demands() {
        let mut public_graph = LoadedProgramFixture::new("main.nia", "module child;");
        let public_entry = public_graph.entry_id();
        public_graph.add_child_with_visibility(
            public_entry,
            "child",
            nia_ids::Visibility::Public,
            "child.nia",
            "pub fn value() i32 { 1 }",
        );
        let mut private_graph = LoadedProgramFixture::new("main.nia", "module child;");
        let private_entry = private_graph.entry_id();
        private_graph.add_child_with_visibility(
            private_entry,
            "child",
            nia_ids::Visibility::Private,
            "child.nia",
            "pub fn value() i32 { 1 }",
        );
        let program_sources = crate::frontend_program_source_fingerprint(
            public_graph.graph.modules().map(|module| {
                (
                    &module.stable_key,
                    crate::source_content_fingerprint("same exact source"),
                    17,
                )
            }),
        );
        let revision = crate::ProviderFactRevision::new_store();
        let empty = crate::ProviderFactSnapshot::empty(revision);
        let public = check_certificate_input_fingerprint(
            program_sources,
            &public_graph.graph.clone().into(),
            &empty,
        );
        let private = check_certificate_input_fingerprint(
            program_sources,
            &private_graph.graph.clone().into(),
            &empty,
        );
        assert_ne!(public, private);

        let demanded = crate::ProviderFactSnapshot::new(
            revision,
            revision,
            [crate::ProviderDemand {
                source_path: SourcePath::new("child.nia"),
                request: crate::ProviderRequest::ModuleBody {
                    module_path: SourcePath::new("child.nia"),
                },
            }],
        );
        assert_ne!(
            public,
            check_certificate_input_fingerprint(
                program_sources,
                &public_graph.graph.into(),
                &demanded,
            )
        );
    }

    #[test]
    fn compiler_inputs_preserve_provider_fact_revision() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let mut program = fixture.program();
        let revision = crate::ProviderFactRevision::new_store().next();
        program.provider_fact_revision = revision;
        let database = CompilerDatabase::new(CompileRequest::new(program));

        assert_eq!(
            database.provider_fact_revision().expect("revision"),
            revision
        );
    }

    #[test]
    fn executable_products_depend_on_incremental_worklists() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let revision = crate::ProviderFactRevision::new_store();
        let mut program = fixture.program();
        program.provider_fact_revision = revision;
        let database = CompilerDatabase::new(CompileRequest::new(program));
        assert_eq!(std::mem::size_of::<ProviderFactWorklistQuery>(), 0);
        assert_eq!(std::mem::size_of::<BodyActivationWorklistQuery>(), 0);
        assert_eq!(std::mem::size_of::<ExecutableFactEpochQuery>(), 0);

        let _ = database.executable_provider_demands();
        let modules = database.db.expect_get(ExecutableCheckedModulesQuery);
        assert!(!modules.is_empty());
        assert_eq!(
            database.provider_fact_revision().expect("revision"),
            revision
        );
        assert_eq!(
            database
                .db
                .context()
                .executable_fact_session
                .lock()
                .expect("executable fact session lock poisoned")
                .applied_provider_fact_revision,
            Some(revision)
        );

        let dependencies = &database.query_trace().dependencies;
        for product in [
            "executable_provider_demands",
            "executable_checked_module_facts",
        ] {
            for worklist in ["body_activation_worklist", "provider_fact_worklist"] {
                assert!(dependencies.iter().any(|dependency| {
                    dependency.from.name == product && dependency.to.name == worklist
                }));
            }
            assert!(dependencies.iter().any(|dependency| {
                dependency.from.name == product && dependency.to.name == "executable_fact_epoch"
            }));
        }
        assert!(dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_checked_modules"
                && dependency.to.name == "executable_checked_module_facts"
        }));
        assert!(dependencies.iter().any(|dependency| {
            dependency.from.name == "provider_fact_revision"
                && dependency.to.name == "provider_fact_worklist"
        }));
    }

    #[test]
    fn executable_products_serialize_the_shared_fact_session() {
        let fixture = LoadedProgramFixture::new("main.nia", "pub fn main() i32 { 0 }");
        let mut program = fixture.program();
        program.runtime = RuntimeModel::FreestandingExecutable;
        let revision = program.provider_fact_revision;
        let db = query_db(program);

        let (_demands, modules) = std::thread::scope(|scope| {
            let demands = scope.spawn(|| db.expect_get(ExecutableProviderDemandsQuery));
            let modules = scope.spawn(|| db.expect_get(ExecutableCheckedModulesQuery));
            (
                demands.join().expect("provider demand query thread"),
                modules.join().expect("checked modules query thread"),
            )
        });

        assert!(!modules.is_empty());
        assert_eq!(
            db.context()
                .executable_fact_session
                .lock()
                .expect("executable fact session lock poisoned")
                .applied_provider_fact_revision,
            Some(revision)
        );
    }

    #[test]
    fn provider_worklist_fingerprint_is_deterministic_and_order_independent() {
        let revision = crate::ProviderFactRevision::new_store().next();
        let method = crate::ProviderDemand {
            source_path: SourcePath::new("main.nia"),
            request: crate::ProviderRequest::Method {
                target_type_name: Some(sym("Thing")),
                method_name: sym("run"),
            },
        };
        let trait_impl = crate::ProviderDemand {
            source_path: SourcePath::new("provider.nia"),
            request: crate::ProviderRequest::TraitImpl {
                trait_name: sym("Display"),
            },
        };
        let first_provider = crate::ProviderFactSnapshot::new(
            revision,
            revision,
            [method.clone(), trait_impl.clone()],
        );
        let mut reversed_changes = HashSet::new();
        reversed_changes.insert(trait_impl);
        reversed_changes.insert(method);
        let second_provider =
            crate::ProviderFactSnapshot::new(revision, revision, reversed_changes);
        assert_eq!(
            provider_fact_worklist_fingerprint(&first_provider),
            provider_fact_worklist_fingerprint(&second_provider)
        );
    }

    #[test]
    fn executable_fact_epoch_defers_full_reset_to_query_boundary() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let database = fixture.database();
        let first_epoch = database.db.expect_get(ExecutableFactEpochQuery);
        let _ = database.db.expect_get(ExecutableCheckedModulesQuery);
        let _ = database.executable_provider_demands();
        let sentinel = crate::ProviderDemand {
            source_path: SourcePath::new("main.nia"),
            request: crate::ProviderRequest::Method {
                target_type_name: None,
                method_name: SymbolId::default(),
            },
        };
        {
            let mut session = database
                .db
                .context()
                .executable_fact_session
                .lock()
                .expect("executable fact session lock poisoned");
            assert_eq!(session.epoch.as_ref(), Some(first_epoch.as_ref()));
            session.applied_provider_changes.insert(sentinel.clone());
        }

        let mut reset = fixture.program();
        reset.runtime = RuntimeModel::FreestandingExecutable;
        database.update(CompileRequest::new(reset));
        {
            let session = database
                .db
                .context()
                .executable_fact_session
                .lock()
                .expect("executable fact session lock poisoned");
            assert_eq!(session.epoch.as_ref(), Some(first_epoch.as_ref()));
            assert!(session.applied_provider_changes.contains(&sentinel));
        }

        let _ = database.executable_provider_demands();
        let latest_epoch = database.db.expect_get(ExecutableFactEpochQuery);
        let session = database
            .db
            .context()
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned");
        assert_ne!(first_epoch.as_ref(), latest_epoch.as_ref());
        assert_eq!(session.epoch.as_ref(), Some(latest_epoch.as_ref()));
        assert!(!session.applied_provider_changes.contains(&sentinel));
    }

    #[test]
    fn provider_revision_update_invalidates_executable_products() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let revision = crate::ProviderFactRevision::new_store();
        let mut program = fixture.program();
        program.provider_fact_revision = revision;
        let database = CompilerDatabase::new(CompileRequest::new(program));
        let _ = database.executable_provider_demands();
        let first_set = database.db.expect_get(ExecutableCheckedModulesQuery);
        assert_eq!(
            database.provider_fact_revision().expect("revision"),
            revision
        );

        let invalidation = database.replace_provider_facts(crate::ProviderFactSnapshot::new(
            revision.next(),
            revision,
            std::iter::empty(),
        ));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        for name in [
            "provider_fact_revision",
            "provider_fact_worklist",
            "executable_provider_demands",
            "executable_checked_modules",
        ] {
            assert!(invalidated.contains(&name), "{invalidated:?}");
        }
        assert!(
            !invalidated.contains(&"body_activation_worklist"),
            "{invalidated:?}"
        );
        assert_eq!(
            database.provider_fact_revision().expect("updated revision"),
            revision.next()
        );
        let revision_query = database
            .query_trace()
            .queries
            .into_iter()
            .find(|query| query.frame.name == "provider_fact_revision")
            .expect("provider fact revision query trace");
        assert_eq!(revision_query.stats.validations, 1);
        assert_eq!(revision_query.stats.green_validations, 0);
        let second_set = database.db.expect_get(ExecutableCheckedModulesQuery);
        assert!(!Arc::ptr_eq(&first_set, &second_set));
        assert!(!second_set.is_empty());
    }

    #[test]
    fn provider_worklist_accumulates_until_consumed() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let revision = crate::ProviderFactRevision::new_store();
        let mut program = fixture.program();
        program.provider_fact_revision = revision;
        let database = CompilerDatabase::new(CompileRequest::new(program));
        let first_demand = crate::ProviderDemand {
            source_path: SourcePath::new("main.nia"),
            request: crate::ProviderRequest::Method {
                target_type_name: None,
                method_name: SymbolId::default(),
            },
        };
        let second_demand = crate::ProviderDemand {
            source_path: SourcePath::new("main.nia"),
            request: crate::ProviderRequest::TraitImpl {
                trait_name: SymbolId::default(),
            },
        };
        let first_revision = revision.next();
        let second_revision = first_revision.next();

        database.replace_provider_facts(crate::ProviderFactSnapshot::new(
            first_revision,
            revision,
            [first_demand.clone()],
        ));
        database.replace_provider_facts(crate::ProviderFactSnapshot::new(
            second_revision,
            revision,
            [first_demand.clone(), second_demand.clone()],
        ));

        let worklist = database.db.expect_get(ProviderFactWorklistQuery);
        let expected_changes = HashSet::from([first_demand, second_demand]);
        assert_eq!(worklist.revision(), second_revision);
        assert_eq!(worklist.demands(), &expected_changes);

        let mut session = ExecutableFactSession::default();
        session.apply_provider_fact_worklist(&worklist, &database.db.context().type_store);
        assert_eq!(
            session.applied_provider_fact_revision,
            Some(second_revision)
        );
        assert_eq!(session.applied_provider_changes, expected_changes);

        let reset_revision = second_revision.next();
        database.replace_provider_facts(crate::ProviderFactSnapshot::new(
            reset_revision,
            reset_revision,
            std::iter::empty(),
        ));
        let reset = database.db.expect_get(ProviderFactWorklistQuery);
        assert_eq!(reset.revision(), reset_revision);
        assert!(reset.demands().is_empty());
    }

    #[test]
    fn provider_worklist_reset_watermark_survives_skipped_revisions() {
        let initial_revision = crate::ProviderFactRevision::new_store();
        let reset_revision = initial_revision.next();
        let current_revision = reset_revision.next();
        let stale = crate::ProviderDemand {
            source_path: SourcePath::new("stale.nia"),
            request: crate::ProviderRequest::TraitImpl {
                trait_name: sym("Stale"),
            },
        };
        let current = crate::ProviderDemand {
            source_path: SourcePath::new("current.nia"),
            request: crate::ProviderRequest::TraitImpl {
                trait_name: sym("Current"),
            },
        };
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let database = fixture.database();
        let mut session = ExecutableFactSession {
            applied_provider_fact_revision: Some(initial_revision),
            applied_provider_changes: HashSet::from([stale.clone()]),
            ..ExecutableFactSession::default()
        };

        session.apply_provider_fact_worklist(
            &crate::ProviderFactSnapshot::new(current_revision, reset_revision, [current.clone()]),
            &database.db.context().type_store,
        );

        assert_eq!(
            session.applied_provider_fact_revision,
            Some(current_revision)
        );
        assert_eq!(session.applied_provider_changes, HashSet::from([current]));
        assert!(!session.applied_provider_changes.contains(&stale));
    }

    #[test]
    fn body_activation_worklist_accumulates_until_consumed() {
        let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let entry_id = fixture.entry_id();
        let first_module = fixture.add_shallow_child(
            entry_id,
            "first",
            "main/first.nia",
            "pub fn first() i32 { 1 }",
        );
        let second_module = fixture.add_shallow_child(
            entry_id,
            "second",
            "main/second.nia",
            "pub fn second() i32 { 2 }",
        );
        assert!(fixture.graph.mark_semantic_selected(first_module));
        assert!(fixture.graph.mark_semantic_selected(second_module));
        let database = fixture.database();
        let _ = database.executable_provider_demands();

        assert!(fixture.graph.mark_process_used_paths(first_module));
        database.update(CompileRequest::new(fixture.program()));

        assert!(fixture.graph.mark_process_used_paths(second_module));
        database.update(CompileRequest::new(fixture.program()));

        let worklist = database.db.expect_get(BodyActivationWorklistQuery);
        let expected = HashMap::from([
            (
                fixture
                    .graph
                    .stable_key(entry_id)
                    .expect("entry stable key")
                    .clone(),
                entry_id,
            ),
            (
                fixture
                    .graph
                    .stable_key(first_module)
                    .expect("first stable key")
                    .clone(),
                first_module,
            ),
            (
                fixture
                    .graph
                    .stable_key(second_module)
                    .expect("second stable key")
                    .clone(),
                second_module,
            ),
        ]);
        assert_eq!(worklist.modules.as_ref(), &expected);

        let _ = database.executable_provider_demands();
        let session = database
            .db
            .context()
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned");
        assert_eq!(
            session.applied_body_activations,
            expected.keys().cloned().collect()
        );
    }

    #[test]
    fn content_identical_input_replacement_keeps_executable_facts_green() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let revision = crate::ProviderFactRevision::new_store();
        let mut program = fixture.program();
        program.provider_fact_revision = revision;
        let database = CompilerDatabase::new(CompileRequest::new(program));
        let first_set = database.db.expect_get(ExecutableCheckedModulesQuery);
        let _ = database.executable_provider_demands();
        let before_update = database.query_trace();

        let invalidation = database.update(
            CompileRequest::new(fixture.program()).with_timings(crate::TimingMode::Summary),
        );

        assert!(
            invalidation.invalidated.is_empty(),
            "{:?}",
            invalidation.invalidated
        );
        let second_set = database.db.expect_get(ExecutableCheckedModulesQuery);
        let _ = database.executable_provider_demands();
        assert!(Arc::ptr_eq(&first_set, &second_set));
        assert!(!second_set.is_empty());
        let after_reuse = database.query_trace();
        for name in [
            "body_activation_worklist",
            "executable_checked_modules",
            "executable_fact_epoch",
            "executable_provider_demands",
            "provider_fact_worklist",
        ] {
            assert_query_executions_unchanged(&before_update, &after_reuse, name);
        }
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
    fn semantic_provider_activation_preserves_resolved_caller_facts() {
        let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let entry_id = fixture.entry_id();
        let revision = crate::ProviderFactRevision::new_store();
        let mut program = fixture.program();
        program.provider_fact_revision = revision;
        let database = CompilerDatabase::new(CompileRequest::new(program));
        let _ = database.executable_provider_demands();
        fixture.add_child(
            entry_id,
            "provider",
            "main/provider.nia",
            "pub fn value() i32 { 1 }",
        );
        let provider_change = crate::ProviderDemand {
            source_path: SourcePath::new("main.nia"),
            request: crate::ProviderRequest::ModuleSemantic {
                module_path: SourcePath::new("main/provider.nia"),
            },
        };
        let checked_function = {
            let mut session = database
                .db
                .context()
                .executable_fact_session
                .lock()
                .expect("executable fact session lock poisoned");
            let state = session
                .modules
                .get_mut(&entry_id)
                .expect("entry executable facts");
            let checked_function = *state
                .checked_functions
                .iter()
                .next()
                .expect("checked entry function");
            state
                .provider_demands_by_function
                .entry(checked_function)
                .or_default()
                .insert(provider_change.clone());
            state.provider_demands.insert(provider_change.clone());
            checked_function
        };

        database.update(CompileRequest::new(fixture.program()));
        database.replace_provider_facts(crate::ProviderFactSnapshot::new(
            revision.next(),
            revision,
            [provider_change],
        ));
        let _ = database.executable_provider_demands();

        let session = database
            .db
            .context()
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned");
        let state = session
            .modules
            .get(&entry_id)
            .expect("preserved entry executable facts");
        assert!(state.checked_functions.contains(&checked_function));
        assert_eq!(
            session.applied_provider_fact_revision,
            Some(revision.next())
        );
        assert!(
            session
                .caches
                .body_resolution_inputs
                .borrow()
                .contains_key(&entry_id)
        );
    }

    #[test]
    fn method_provider_change_removes_only_affected_function_diagnostics() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "struct Value {} fn helper() i32 { 1 } fn main(value: Value) i32 { value.missing() }",
        );
        let entry_id = fixture.entry_id();
        let revision = crate::ProviderFactRevision::new_store();
        let mut program = fixture.program();
        program.provider_fact_revision = revision;
        let database = CompilerDatabase::new(CompileRequest::new(program));
        let provider_changes = database
            .executable_provider_demands()
            .expect("test executable provider demands")
            .into_iter()
            .filter(|demand| matches!(demand.request, crate::ProviderRequest::Method { .. }))
            .collect::<Vec<_>>();
        assert!(!provider_changes.is_empty());
        let (affected_function, unaffected_function) = {
            let session = database
                .db
                .context()
                .executable_fact_session
                .lock()
                .expect("executable fact session lock poisoned");
            let state = session.modules.get(&entry_id).expect("entry facts");
            assert!(!state.diagnostics.is_empty());
            let affected = *state
                .provider_demands_by_function
                .iter()
                .find(|(_, demands)| {
                    demands
                        .iter()
                        .any(|demand| provider_changes.contains(demand))
                })
                .map(|(function, _)| function)
                .expect("function-owned method demand");
            let unaffected = *state
                .checked_functions
                .iter()
                .find(|function| **function != affected)
                .expect("unaffected helper function");
            (affected, unaffected)
        };

        fixture.add_child(
            entry_id,
            "provider",
            "main/provider.nia",
            "pub fn value() i32 { 1 }",
        );
        database.update(CompileRequest::new(fixture.program()));
        database.replace_provider_facts(crate::ProviderFactSnapshot::new(
            revision.next(),
            revision,
            provider_changes,
        ));
        let worklist = database.db.expect_get(ProviderFactWorklistQuery);

        let mut session = database
            .db
            .context()
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned");
        session.apply_provider_fact_worklist(&worklist, &database.db.context().type_store);
        let state = session
            .modules
            .get(&entry_id)
            .expect("partially retained entry facts");
        assert!(!state.checked_functions.contains(&affected_function));
        assert!(state.checked_functions.contains(&unaffected_function));
        assert!(state.diagnostics.is_empty(), "{:?}", state.diagnostics);
        assert_eq!(state.diagnostic_owners.len(), state.diagnostics.len());
        assert!(
            session
                .caches
                .body_resolution_inputs
                .borrow()
                .contains_key(&entry_id)
        );
    }

    #[test]
    fn revision_only_update_refreshes_all_revision_bearing_products() {
        let source = "pub struct S { value: i32 } fn main() i32 { let value: i32 = 0; value }";
        let mut fixture = LoadedProgramFixture::new("main.nia", source);
        let module_id = fixture.entry_id();
        let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
        let first_tree = database
            .db
            .expect_get(DeclarationModuleItemTreeInputQuery(module_id));
        let first_defs = database.db.expect_get(ModuleDefsQuery(module_id));
        assert!(
            first_tree
                .items
                .iter()
                .all(|item| item.node_key.revision == SourceRevision::INITIAL)
        );
        assert!(
            first_defs
                .semantic
                .def_nodes
                .entries()
                .all(|(key, _)| key.revision == SourceRevision::INITIAL)
        );

        fixture.update_module_source(module_id, source, SourceRevision(1));
        database.update(CompileRequest::new(fixture.program()));
        let before_second_check = database.query_trace();

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        let latest_tree = database
            .db
            .expect_get(DeclarationModuleItemTreeInputQuery(module_id));
        let latest_defs = database.db.expect_get(ModuleDefsQuery(module_id));
        let after_second_check = database.query_trace();

        assert!(!Arc::ptr_eq(&first_tree, &latest_tree));
        assert!(!Arc::ptr_eq(&first_defs, &latest_defs));
        assert!(
            latest_tree
                .items
                .iter()
                .all(|item| item.node_key.revision == SourceRevision(1))
        );
        assert!(
            latest_defs
                .semantic
                .def_nodes
                .entries()
                .all(|(key, _)| key.revision == SourceRevision(1))
        );
        assert!(
            query_executions(&before_second_check, "declaration_type_lowering")
                < query_executions(&after_second_check, "declaration_type_lowering")
        );
        assert!(
            query_executions(&before_second_check, "item_signatures")
                < query_executions(&after_second_check, "item_signatures")
        );
    }

    #[test]
    fn type_store_preserves_published_slots_across_database_updates() {
        let mut fixture = LoadedProgramFixture::new("main.nia", "pub struct S { value: i32 }");
        let module_id = fixture.entry_id();
        let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));
        let first_lowering = database.db.expect_get(TypeLoweringQuery(module_id));
        let type_store = &database.db.context().type_store;
        let first_i32 = type_store
            .append_for_module(module_id)
            .intern(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I32));
        assert!(
            first_lowering
                .semantic
                .explicit_type_roots()
                .into_iter()
                .all(|ty| type_store.get(ty).is_some())
        );

        fixture.update_module_source(
            module_id,
            "pub struct S { value: i32, flag: &bool }",
            SourceRevision(1),
        );
        database.update(CompileRequest::new(fixture.program()));
        let second_lowering = database.db.expect_get(TypeLoweringQuery(module_id));

        assert_eq!(
            type_store.get(first_i32),
            Some(&nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I32))
        );
        assert!(
            second_lowering
                .semantic
                .explicit_type_roots()
                .into_iter()
                .any(|ty| {
                    matches!(
                        type_store.get(ty),
                        Some(nia_ty::TyKind::Pointer { elem, .. })
                            if matches!(
                                type_store.get(*elem),
                                Some(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::Bool))
                            )
                    )
                })
        );
    }

    #[test]
    fn type_normalization_appends_to_the_session_type_store() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "type ByteRef = &u8; pub fn read(value: ByteRef) u8 { 0 }",
        );
        let module_id = fixture.entry_id();
        let database = fixture.database();
        let lowering = database.db.expect_get(TypeLoweringQuery(module_id));
        let normalization = database.db.expect_get(TypeNormalizationQuery(module_id));
        let type_store = &database.db.context().type_store;

        for ty_id in lowering.semantic.explicit_type_roots() {
            assert!(type_store.get(ty_id).is_some());
        }
        for normalized in normalization.semantic.normalized.values() {
            assert!(type_store.get(*normalized).is_some());
        }
        assert!(
            normalization
                .semantic
                .normalized
                .iter()
                .any(|(source, normalized)| source != normalized)
        );
    }

    #[test]
    fn const_phases_publish_synthesized_types_to_canonical_store() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
const values = 0usize..3usize;
const width: usize = values.end();

fn main() i32 { 0 }
"#,
        );
        let module_id = fixture.entry_id();
        let database = fixture.database();
        let lowering = database.db.expect_get(TypeLoweringQuery(module_id));
        let _ = database.db.expect_get(TypeNormalizationQuery(module_id));

        let _ = database.db.expect_get(ConstArrayLengthsQuery(module_id));
        let _ = database.db.expect_get(ConstEnumValuesQuery(module_id));
        let values = database.db.expect_get(ConstValuesQuery(module_id));
        let _ = database.db.expect_get(ConstTypedFactsQuery(module_id));
        let _ = database.db.expect_get(ConstQuery(module_id));

        for ty in lowering.semantic.explicit_type_roots() {
            assert!(database.db.context().type_store.get(ty).is_some());
        }
        let range_ty = values
            .typed_values
            .values()
            .filter_map(|value| value.ty.runtime())
            .find(|ty| {
                matches!(
                    database.db.context().type_store.get(*ty),
                    Some(nia_ty::TyKind::Range { .. })
                )
            })
            .expect("const range type published to canonical store");
        assert!(database.db.context().type_store.get(range_ty).is_some());
    }

    #[test]
    fn body_check_publishes_synthesized_types_to_canonical_store() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
fn main() i32 {
    let values = [1i32, 2i32, 3i32];
    values[0]
}
"#,
        );
        let module_id = fixture.entry_id();
        let database = fixture.database();
        let _ = database.db.expect_get(ConstQuery(module_id));

        let body = database.db.expect_get(BodyCheckQuery(module_id));

        assert!(body.semantic.facts.function_facts.values().any(|facts| {
            facts.local_types.values().any(|ty| {
                matches!(
                    database.db.context().type_store.get(*ty),
                    Some(nia_ty::TyKind::Array {
                        len: nia_ty::ArrayLenTy::ConstValue(3),
                        ..
                    })
                )
            })
        }));
    }

    #[test]
    fn signature_and_full_normalization_share_ids_in_either_query_order() {
        fn assert_order(signature_first: bool) {
            let fixture = LoadedProgramFixture::new(
                "main.nia",
                "type Ref[T] = &T; pub fn read(value: Ref[u16]) u16 { 0 }",
            );
            let module_id = fixture.entry_id();
            let database = fixture.database();
            let signature_key = SignatureTypeNormalizationQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Functions,
            );
            let (signature, full) = if signature_first {
                let signature = database.db.expect_get(signature_key);
                let full = database.db.expect_get(TypeNormalizationQuery(module_id));
                (signature, full)
            } else {
                let full = database.db.expect_get(TypeNormalizationQuery(module_id));
                let signature = database.db.expect_get(signature_key);
                (signature, full)
            };

            assert!(
                signature
                    .semantic
                    .normalized
                    .values()
                    .chain(full.semantic.normalized.values())
                    .all(|ty| database.db.context().type_store.get(*ty).is_some())
            );
            let shared_alias_expansions = signature
                .semantic
                .normalized
                .iter()
                .filter(|(source, normalized)| {
                    source != normalized && full.semantic.normalized.get(source) == Some(normalized)
                })
                .count();
            assert!(
                shared_alias_expansions > 0,
                "signature/full normalization did not share an alias expansion"
            );
        }

        assert_order(true);
        assert_order(false);
    }

    #[test]
    fn type_store_isolates_compiler_database_handle_identity() {
        let first_fixture = LoadedProgramFixture::new("main.nia", "pub struct S { value: i32 }");
        let second_fixture = LoadedProgramFixture::new("main.nia", "pub struct S { value: i32 }");
        let first_module_id = first_fixture.entry_id();
        let second_module_id = second_fixture.entry_id();
        let first = first_fixture.database();
        let second = second_fixture.database();
        let _ = first.db.expect_get(TypeLoweringQuery(first_module_id));
        let _ = second.db.expect_get(TypeLoweringQuery(second_module_id));
        let first_store = &first.db.context().type_store;
        let second_store = &second.db.context().type_store;
        let first_i32 = first_store
            .append_for_module(first_module_id)
            .intern(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I32));
        let second_i32 = second_store
            .append_for_module(second_module_id)
            .intern(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I32));

        assert_ne!(first_store.id(), second_store.id());
        assert_ne!(first_i32, second_i32);
        assert_eq!(first_store.get(second_i32), None);
        assert_eq!(second_store.get(first_i32), None);
    }

    #[test]
    fn randomized_incremental_checks_match_clean_recomputation() {
        #[derive(Debug, PartialEq)]
        struct ObservableCheck {
            diagnostics: Vec<ProgramDiagnostic>,
            modules: Vec<(String, usize, usize, usize, usize)>,
        }

        fn observable(program: CheckedProgramAnalysis) -> ObservableCheck {
            ObservableCheck {
                diagnostics: program.diagnostics,
                modules: program
                    .modules
                    .iter()
                    .map(|module| {
                        (
                            module.path.as_str().to_owned(),
                            module.defs.defs.iter().count(),
                            module.body_ir.function_bodies.len(),
                            module.semantic_facts.function_facts.len(),
                            module.provider_demands.len(),
                        )
                    })
                    .collect(),
            }
        }

        let sources = [
            "fn main() i32 { 0 }",
            "fn main() i32 { 1 }",
            "fn helper() i32 { 2 } fn main() i32 { helper() }",
            "struct Value { field: i32 } fn main() i32 { let value = Value { field: 3 }; value.field }",
            "fn main() i32 { true }",
            "fn main() i32 { let value: i32 = 4; value }",
            "const answer: i32 = 5; fn main() i32 { answer }",
            "fn main() i32 { missing() }",
        ];
        let mut fixture = LoadedProgramFixture::new("main.nia", sources[0]);
        let module_id = fixture.entry_id();
        let incremental = fixture.database();
        let mut random = 0x9e37_79b9_u32;

        for revision in 1..=24_u64 {
            random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let source = sources[(random as usize) % sources.len()];
            fixture.update_module_source(module_id, source, SourceRevision(revision));
            incremental.update(CompileRequest::new(fixture.program()));
            let incremental_output = observable(incremental.analyze_program());

            let clean_fixture = LoadedProgramFixture::new("main.nia", source);
            let clean_output = observable(clean_fixture.database().analyze_program());

            assert_eq!(
                incremental_output, clean_output,
                "incremental/clean mismatch at revision {revision} for `{source}`"
            );
        }
    }

    #[test]
    fn function_body_update_refreshes_handles_but_keeps_public_snapshots_green() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "pub struct S { value: i32 } fn main() i32 { 0 }",
        );
        let module_id = fixture.entry_id();
        let database = fixture.database();

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
        let first_using_scope = database.db.expect_get(ModuleUsingScopeQuery(module_id));

        fixture.update_module_source(
            module_id,
            "pub struct S { value: i32 } fn main() i32 { 1 }",
            SourceRevision(1),
        );
        database.update(CompileRequest::new(fixture.program()));

        let second = database.analyze_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        let latest_using_scope = database.db.expect_get(ModuleUsingScopeQuery(module_id));
        assert!(Arc::ptr_eq(&first_using_scope, &latest_using_scope));
    }

    #[test]
    fn function_body_type_update_refreshes_revision_bearing_signature_queries() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "pub struct S { value: i32 } fn main() i32 { let value: i32 = 0; value }",
        );
        let module_id = fixture.entry_id();
        let database = fixture.database();

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        fixture.update_module_source(
            module_id,
            "pub struct S { value: i32 } fn main() i32 { let value: u8 = 0; value as i32 }",
            SourceRevision(1),
        );
        database.update(CompileRequest::new(fixture.program()));

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
    }

    #[test]
    fn body_local_type_update_reuses_program_body_signature_indexes() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "pub struct S { value: i32 } fn main() i32 { let value: i32 = 0; value }",
        );
        let module_id = fixture.entry_id();
        let database = fixture.database();

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        fixture.update_module_source(
            module_id,
            "pub struct S { value: i32 } fn main() i32 { let value: u8 = 0; value as i32 }",
            SourceRevision(1),
        );
        database.update(CompileRequest::new(fixture.program()));
        let before_second_check = database.query_trace();

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        let after_second_check = database.query_trace();

        assert_query_executions_unchanged(
            &before_second_check,
            &after_second_check,
            "extension_provider_discovery_index",
        );
    }

    #[test]
    fn function_signature_update_refreshes_revision_bearing_definition_queries() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "pub struct S { value: i32 } fn helper() i32 { 1 } fn main() i32 { helper() }",
        );
        let module_id = fixture.entry_id();
        let database = fixture.database();

        let first = database.codegen_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        fixture.update_module_source(
            module_id,
            "pub struct S { value: i32 } fn helper() u8 { 1 } fn main() i32 { helper() as i32 }",
            SourceRevision(1),
        );
        database.update(CompileRequest::new(fixture.program()));
    }

    #[test]
    fn function_body_type_update_refreshes_signature_program_type_context() {
        let mut fixture =
            LoadedProgramFixture::new("main.nia", "fn main() i32 { let value: i32 = 0; value }");
        let entry_id = fixture.entry_id();
        fixture.add_child(entry_id, "helper", "helper.nia", "fn helper() i32 { 1 }");
        let database = fixture.database();

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        fixture.update_module_source(
            entry_id,
            "fn main() i32 { let value: u8 = 0; value as i32 }",
            SourceRevision(1),
        );
        database.update(CompileRequest::new(fixture.program()));
        let before_second_check = database.query_trace();

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        let after_second_check = database.query_trace();

        assert!(
            query_executions(&before_second_check, "signature_type_normalization")
                < query_executions(&after_second_check, "signature_type_normalization")
        );
    }

    #[test]
    fn source_identity_update_invalidates_module_dependent_queries() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "pub struct S { value: i32 } fn main() i32 { 0 }",
        );
        let module_id = fixture.entry_id();
        let database = fixture.database();

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        fixture.update_module_path(module_id, "renamed.nia");
        database.update(CompileRequest::new(fixture.program()));

        let second = database.analyze_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        assert_eq!(second.modules[0].path.as_str(), "renamed.nia");
    }

    #[test]
    fn source_identity_change_invalidates_loaded_module_list() {
        let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let module_id = fixture.entry_id();
        let database = fixture.database();
        let _ = database.check_program();

        fixture.update_module_path(module_id, "other.nia");
        database.update(CompileRequest::new(fixture.program()));
        let loaded = database.db.expect_get(LoadedModulesQuery);
        assert_eq!(
            resolve_stable_module_sequence(&database.db, &loaded).expect("renamed module sequence"),
            vec![module_id]
        );
        assert_eq!(
            database.db.expect_get(ModulePathQuery(module_id)).as_str(),
            "other.nia"
        );
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
    fn parse_error_changes_keep_stable_program_module_membership_green() {
        let mut fixture =
            LoadedProgramFixture::new("main.nia", "module broken; fn main() i32 { 0 }");
        let entry_id = fixture.entry_id();
        let broken_id =
            fixture.add_child(entry_id, "broken", "broken.nia", "fn helper() i32 { 1 }");
        let mut initial = fixture.program();
        initial
            .modules
            .iter_mut()
            .find(|module| module.id == broken_id)
            .expect("broken fixture module")
            .parse_errors = vec![ParseError {
            span: Span::default(),
            message: "first parse failure".to_string(),
            node_key: None,
        }];
        let database = CompilerDatabase::new(CompileRequest::new(initial));
        let first = database.db.expect_get(ProgramSignatureModuleIdsQuery(
            nia_item_tree::SignatureItemSet::Functions,
        ));
        assert_eq!(
            resolve_stable_module_sequence(&database.db, &first)
                .expect("program signature module sequence"),
            vec![entry_id]
        );
        let mut updated = fixture.program();
        updated
            .modules
            .iter_mut()
            .find(|module| module.id == broken_id)
            .expect("broken fixture module")
            .parse_errors = vec![ParseError {
            span: Span::default(),
            message: "second parse failure".to_string(),
            node_key: None,
        }];

        database.update(CompileRequest::new(updated));

        let latest = database.db.expect_get(ProgramSignatureModuleIdsQuery(
            nia_item_tree::SignatureItemSet::Functions,
        ));
        assert!(Arc::ptr_eq(&first, &latest));
        let trace = database.query_trace();
        let module_ids = trace
            .queries
            .iter()
            .find(|query| query.frame.name == "program_signature_module_ids")
            .expect("program signature module ids trace");
        assert_eq!(module_ids.stats.executions, 1);
        assert_eq!(module_ids.stats.validations, 1);
        assert_eq!(module_ids.stats.green_validations, 1);
    }

    #[test]
    fn signature_changes_keep_stable_program_module_membership_green() {
        let mut fixture =
            LoadedProgramFixture::new("main.nia", "fn first() i32 { 1 } fn main() i32 { first() }");
        let module_id = fixture.entry_id();
        let database = fixture.database();
        let first = database.db.expect_get(ProgramSignatureModuleIdsQuery(
            nia_item_tree::SignatureItemSet::Functions,
        ));

        fixture.update_module_source(
            module_id,
            "fn first() i32 { 1 } fn second() i32 { 2 } fn main() i32 { first() }",
            SourceRevision(1),
        );
        database.update(CompileRequest::new(fixture.program()));

        let latest = database.db.expect_get(ProgramSignatureModuleIdsQuery(
            nia_item_tree::SignatureItemSet::Functions,
        ));
        assert!(Arc::ptr_eq(&first, &latest));
        let trace = database.query_trace();
        let module_ids = trace
            .queries
            .iter()
            .find(|query| query.frame.name == "program_signature_module_ids")
            .expect("program signature module ids trace");
        assert_eq!(module_ids.stats.executions, 1);
        assert_eq!(module_ids.stats.validations, 1);
        assert_eq!(module_ids.stats.green_validations, 1);
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
    fn public_surface_snapshots_are_query_derived_facts() {
        let fixture = LoadedProgramFixture::new("main.nia", "pub struct S { value: i32 }");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let _ = db.expect_get(PublicSurfacesQuery);
        let _ = db.expect_get(ModulePublicSurfaceQuery(module_id));
        let _ = db.expect_get(ModuleUsingScopeQuery(module_id));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "public_surfaces"
                && dependency.to.name == "public_surface_module_facts"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            matches!(
                dependency.from.name,
                "public_surfaces" | "public_using_scopes" | "public_surface_module_facts"
            ) && dependency.to.name == "module_defs"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "public_surfaces" && dependency.to.name == "module_graph"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "public_using_scopes" && dependency.to.name == "public_surfaces"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "public_using_scopes"
                && dependency.to.name == "public_surface_module_facts"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "module_using_scope"
                && dependency.to.name == "public_using_scopes"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "module_public_surface"
                && dependency.to.name == "public_surfaces"
        }));
    }

    #[test]
    fn item_tree_queries_reuse_single_layer_product_handles() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 1 }");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let module_input: Arc<ModuleItemTree> = db.expect_get(ModuleItemTreeInputQuery(module_id));
        let active_input: Arc<ActiveModuleItemTree> =
            db.expect_get(ActiveModuleItemTreeInputQuery(module_id));
        let full_module: Arc<ModuleItemTree> = db.expect_get(FullModuleItemTreeQuery(module_id));
        let full_active: Arc<ActiveModuleItemTree> =
            db.expect_get(FullActiveModuleItemTreeQuery(module_id));

        let module_input_batch = db
            .get_many([ModuleItemTreeInputQuery(module_id)])
            .expect("module input batch should succeed");
        let active_input_batch = db
            .get_many([ActiveModuleItemTreeInputQuery(module_id)])
            .expect("active input batch should succeed");
        let full_module_batch = db
            .get_many([FullModuleItemTreeQuery(module_id)])
            .expect("full module batch should succeed");
        let full_active_batch = db
            .get_many([FullActiveModuleItemTreeQuery(module_id)])
            .expect("full active batch should succeed");

        assert!(Arc::ptr_eq(&module_input, &module_input_batch[0]));
        assert!(Arc::ptr_eq(&active_input, &active_input_batch[0]));
        assert!(Arc::ptr_eq(&full_module, &full_module_batch[0]));
        assert!(Arc::ptr_eq(&full_active, &full_active_batch[0]));
    }

    #[test]
    fn executable_value_refs_resolve_only_the_requested_body_item() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "fn helper() i32 { 1 } fn main() i32 { helper() }",
        );
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());
        let defs = db.expect_get(ModuleDefsQuery(module_id));
        let main = GlobalDefId {
            module_id,
            def_id: defs.semantic.module_scope.values.get(&sym("main")).unwrap(),
        };
        let helper = GlobalDefId {
            module_id,
            def_id: defs
                .semantic
                .module_scope
                .values
                .get(&sym("helper"))
                .unwrap(),
        };

        let edges = db.expect_get(ExecutableValueRefEdgesQuery(main));
        let trace = db.query_trace();

        assert!(edges.functions.contains(&helper), "{:?}", edges.functions);
        assert!(trace_has_dependency(
            &trace,
            "executable_value_ref_edges",
            "executable_value_ref_item"
        ));
        assert!(!trace_has_dependency(
            &trace,
            "executable_value_ref_edges",
            "value_resolution"
        ));
        assert!(trace_has_dependency(
            &trace,
            "executable_value_ref_edges",
            "full_active_module_item_tree"
        ));
        assert!(trace_has_dependency(
            &trace,
            "executable_value_ref_item",
            "executable_value_ref_item_index"
        ));
        assert!(trace_has_dependency(
            &trace,
            "executable_value_ref_item_index",
            "full_active_module_item_tree"
        ));
        assert!(trace_has_dependency(
            &trace,
            "executable_value_ref_item_index",
            "module_defs"
        ));
    }

    #[test]
    fn persistent_check_certificate_reuses_diagnostics_and_verifies_fresh() {
        static CACHE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let cache_id = CACHE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "nia-clean-check-certificate-{}-{cache_id}",
            std::process::id()
        ));
        let compile = |source: &str, verify: bool| {
            let fixture = LoadedProgramFixture::new("main.nia", source);
            let module_id = fixture.entry_id();
            let db = query_db_with_frontend_cache(
                fixture.program(),
                HashMap::from([(
                    module_id,
                    (crate::source_content_fingerprint(source), source.len()),
                )]),
                root.clone(),
                verify,
            );
            let inputs = Arc::clone(&db.context().inputs);
            let database = super::CompilerDatabase { db, inputs };
            let report = database
                .entry_check_program()
                .expect("test entry check report");
            (report, database.query_trace())
        };

        let source = "fn main() i32 { 1 }";
        let (cold, cold_trace) = compile(source, false);
        assert!(cold.diagnostics.is_empty(), "{:?}", cold.diagnostics);
        assert_eq!(query_executions(&cold_trace, "entry_checked_program"), 1);
        assert!(query_executions(&cold_trace, "executable_checked_module_facts") > 0);

        let (warm, warm_trace) = compile(source, false);
        assert_eq!(warm.optimization, cold.optimization);
        assert_eq!(warm.diagnostics, cold.diagnostics);
        assert_eq!(warm.checked_body_count(), cold.checked_body_count());
        assert_eq!(warm.reachable_body_count(), cold.reachable_body_count());
        assert_eq!(
            warm.graph
                .stable_key(warm.graph.entry())
                .expect("warm entry stable key"),
            cold.graph
                .stable_key(cold.graph.entry())
                .expect("cold entry stable key")
        );
        assert!(!warm.graph.ptr_eq(&cold.graph));
        assert_eq!(query_executions(&warm_trace, "entry_checked_program"), 0);
        assert_eq!(
            query_executions(&warm_trace, "executable_checked_module_facts"),
            0
        );
        assert_eq!(query_executions(&warm_trace, "body_check"), 0);

        let (verified, verified_trace) = compile(source, true);
        assert_eq!(verified.optimization, cold.optimization);
        assert_eq!(verified.diagnostics, cold.diagnostics);
        assert_eq!(verified.checked_body_count(), cold.checked_body_count());
        assert_eq!(
            query_executions(&verified_trace, "entry_checked_program"),
            1
        );

        let invalid_source = "fn main() i32 { true }";
        let (edited, edited_trace) = compile(invalid_source, false);
        assert!(!edited.diagnostics.is_empty());
        assert_eq!(query_executions(&edited_trace, "entry_checked_program"), 1);
        let (warm_invalid, warm_invalid_trace) = compile(invalid_source, false);
        assert_eq!(warm_invalid.diagnostics, edited.diagnostics);
        assert_eq!(
            query_executions(&warm_invalid_trace, "entry_checked_program"),
            0
        );

        let invalid_fixture = LoadedProgramFixture::new("main.nia", invalid_source);
        let invalid_module = invalid_fixture.entry_id();
        let invalid_db = query_db_with_frontend_cache(
            invalid_fixture.program(),
            HashMap::from([(
                invalid_module,
                (
                    crate::source_content_fingerprint(invalid_source),
                    invalid_source.len(),
                ),
            )]),
            root.clone(),
            false,
        );
        let invalid_inputs = Arc::clone(&invalid_db.context().inputs);
        let invalid_database = super::CompilerDatabase {
            db: invalid_db,
            inputs: invalid_inputs,
        };
        let invalid_context = invalid_database
            .check_certificate_context(FrontendCheckScope::Entry)
            .expect("certificate context query")
            .expect("invalid-source certificate context");
        invalid_database
            .db
            .context()
            .signature_cache
            .as_ref()
            .expect("signature cache")
            .publish_check_certificate(
                invalid_context.identity(),
                crate::signature_cache::CachedCheckCertificate {
                    checked_body_count: 1,
                    reachable_body_count: 1,
                    diagnostics: Vec::new(),
                },
                true,
            )
            .expect("inject semantically wrong clean certificate");
        let (trusted_invalid, trusted_invalid_trace) = compile(invalid_source, false);
        assert!(trusted_invalid.diagnostics.is_empty());
        assert_eq!(
            query_executions(&trusted_invalid_trace, "entry_checked_program"),
            0
        );
        let (verified_invalid, verified_invalid_trace) = compile(invalid_source, true);
        assert_eq!(verified_invalid.diagnostics, edited.diagnostics);
        assert_eq!(
            query_executions(&verified_invalid_trace, "entry_checked_program"),
            1
        );
        let (retired_invalid, retired_invalid_trace) = compile(invalid_source, false);
        assert_eq!(retired_invalid.diagnostics, verified_invalid.diagnostics);
        assert_eq!(
            query_executions(&retired_invalid_trace, "entry_checked_program"),
            0
        );
        let (_, warm_after_verification) = compile(source, false);
        assert_eq!(
            query_executions(&warm_after_verification, "entry_checked_program"),
            0
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn persistent_executable_value_ref_edges_skip_resolution_and_verify_replacement() {
        static CACHE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let cache_id = CACHE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "nia-executable-value-ref-edges-{}-{cache_id}",
            std::process::id()
        ));
        let source = "fn helper() i32 { 1 } fn main() i32 { helper() }";
        let source_fingerprint = crate::source_content_fingerprint(source);
        let compile = |verify| {
            let fixture = LoadedProgramFixture::new("main.nia", source);
            let module_id = fixture.entry_id();
            let db = query_db_with_frontend_cache(
                fixture.program(),
                HashMap::from([(module_id, (source_fingerprint, source.len()))]),
                root.clone(),
                verify,
            );
            let defs = db.expect_get(ModuleDefsQuery(module_id));
            let owner = GlobalDefId {
                module_id,
                def_id: defs.semantic.module_scope.values.get(&sym("main")).unwrap(),
            };
            let helper = GlobalDefId {
                module_id,
                def_id: defs
                    .semantic
                    .module_scope
                    .values
                    .get(&sym("helper"))
                    .unwrap(),
            };
            let edges = db.expect_get(ExecutableValueRefEdgesQuery(owner));
            (owner, edges.functions.contains(&helper), db.query_trace())
        };

        let (cold_owner, cold_contains_helper, cold) = compile(false);
        assert!(cold_contains_helper);
        assert!(trace_has_dependency(
            &cold,
            "executable_value_ref_edges",
            "executable_value_ref_item"
        ));

        let (_, warm_contains_helper, warm) = compile(false);
        assert!(warm_contains_helper);
        assert!(trace_has_dependency(
            &warm,
            "executable_value_ref_edges",
            "frontend_program_sources"
        ));
        assert!(!trace_has_dependency(
            &warm,
            "executable_value_ref_edges",
            "executable_value_ref_item"
        ));
        assert!(!trace_has_dependency(
            &warm,
            "executable_value_ref_edges",
            "full_active_module_item_tree"
        ));

        let module = StableModuleKey::from_source_identity(SourceIdentity::new("main.nia"));
        let program_sources = crate::frontend_program_source_fingerprint([(
            &module,
            source_fingerprint,
            source.len(),
        )]);
        let namespace =
            crate::FrontendCacheNamespace::new(&TargetConfig::host(), RuntimeModel::Bare);
        let key = crate::FrontendExecutableValueRefEdgesCacheKey::new(
            namespace,
            &module,
            cold_owner.def_id,
            program_sources,
        );
        let cache = crate::signature_cache::PersistentSignatureCache::new(root.clone());
        cache.remove_executable_value_ref_edges(key);
        cache
            .publish_executable_value_ref_edges(
                crate::signature_cache::ExecutableValueRefEdgesIdentity {
                    key,
                    namespace,
                    module: &module,
                    owner: cold_owner.def_id,
                    program_sources,
                },
                &crate::signature_cache::CachedExecutableValueRefEdges::default(),
                &HashMap::from([(cold_owner.module_id, "main.nia".to_string())]),
                false,
            )
            .expect("publish semantically wrong value-ref edges");

        let (_, verified_contains_helper, verified) = compile(true);
        assert!(verified_contains_helper);
        assert!(trace_has_dependency(
            &verified,
            "executable_value_ref_edges",
            "executable_value_ref_item"
        ));

        let (_, replaced_contains_helper, replaced) = compile(false);
        assert!(replaced_contains_helper);
        assert!(!trace_has_dependency(
            &replaced,
            "executable_value_ref_edges",
            "executable_value_ref_item"
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn executable_value_ref_item_refreshes_from_current_module_facts() {
        let source = "fn helper() i32 { 1 } fn main() i32 { helper() }";
        let mut fixture = LoadedProgramFixture::new("main.nia", source);
        let module_id = fixture.entry_id();
        let database = fixture.database();
        let defs = database.db.expect_get(ModuleDefsQuery(module_id));
        let owner = GlobalDefId {
            module_id,
            def_id: defs.semantic.module_scope.values.get(&sym("main")).unwrap(),
        };
        let first = database.db.expect_get(ExecutableValueRefItemQuery(owner));
        assert_eq!(
            first.as_ref().as_ref().unwrap().owner_node_key.revision,
            SourceRevision::INITIAL
        );

        fixture.update_module_source(module_id, source, SourceRevision(1));
        database.update(CompileRequest::new(fixture.program()));

        let latest = database.db.expect_get(ExecutableValueRefItemQuery(owner));
        assert!(!Arc::ptr_eq(&first, &latest));
        assert_eq!(
            latest.as_ref().as_ref().unwrap().owner_node_key.revision,
            SourceRevision(1)
        );
    }

    #[test]
    fn executable_value_refs_include_unqualified_static_uses() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "static mut calls: i32 = 0; fn main() i32 { calls += 1; calls }",
        );
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());
        let defs = db.expect_get(ModuleDefsQuery(module_id));
        let main = GlobalDefId {
            module_id,
            def_id: defs.semantic.module_scope.values.get(&sym("main")).unwrap(),
        };
        let calls = GlobalDefId {
            module_id,
            def_id: defs
                .semantic
                .module_scope
                .values
                .get(&sym("calls"))
                .unwrap(),
        };

        let edges = db.expect_get(ExecutableValueRefEdgesQuery(main));

        assert!(edges.globals.contains(&calls), "{:?}", edges.globals);
    }

    #[test]
    fn module_defs_query_uses_active_item_tree_query() {
        let fixture = LoadedProgramFixture::new("main.nia", "pub struct S { value: i32 }");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let defs = db.expect_get(ModuleDefsQuery(module_id));
        let item_tree = db.expect_get(ActiveModuleItemTreeQuery(module_id));
        let item_node_key = &item_tree.items[0].node_key;
        let item_node_id = defs
            .semantic
            .def_nodes
            .node_id(item_node_key)
            .expect("definition node id");
        let trace = db.query_trace();

        assert_eq!(
            defs.semantic.def_nodes.store_id(),
            db.context().node_store.id()
        );
        assert_eq!(
            db.context().node_store.locator(item_node_id),
            Some(item_node_key.clone())
        );
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "module_defs" && dependency.to.name == "active_module_item_tree"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "module_defs" && dependency.to.name == "module_origins"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "active_module_item_tree"
                && dependency.to.name == "module_item_tree"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "module_item_tree"
                && dependency.to.name == "module_item_tree_input"
        }));
    }

    #[test]
    fn extension_queries_use_module_semantic_queries() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "struct S { value: i32 } extend S { pub fn make(value: i32) S { { value: value } } }",
        );
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let _ = db.expect_get(ExtensionProviderValidationFactsQuery(module_id));
        let _ = db.expect_get(ExtensionProviderModuleFactsQuery(module_id));
        let _ = db.expect_get(ExtensionMethodIndexQuery);
        let _ = db.expect_get(ExtensionProviderDiscoveryIndexQuery);
        let trace = db.query_trace();

        assert!(
            !trace
                .queries
                .iter()
                .any(|query| query.frame.name == "extension_provider_program_facts")
        );
        assert!(trace_has_dependency(
            &trace,
            "extension_provider_validation_facts",
            "extension_signature_module_input"
        ));
        assert!(trace_has_dependency(
            &trace,
            "extension_provider_validation_facts",
            "extension_trait_signature_index"
        ));
        assert!(trace_has_dependency(
            &trace,
            "extension_trait_signature_index",
            "module_program_signature_facts"
        ));
        assert!(trace_has_dependency(
            &trace,
            "module_program_signature_facts",
            "signature_item_signatures"
        ));
        assert!(!trace_has_dependency(
            &trace,
            "module_program_signature_facts",
            "module_defs"
        ));
        assert!(!trace_has_dependency(
            &trace,
            "module_program_signature_facts",
            "signature_type_lowering"
        ));
        assert!(!trace_has_dependency(
            &trace,
            "extension_provider_validation_facts",
            "program_trait_solving_signatures"
        ));
        assert!(trace_has_dependency(
            &trace,
            "extension_signature_module_input",
            "signature_item_signatures"
        ));
        assert!(trace_has_dependency(
            &trace,
            "extension_signature_module_input",
            "signature_type_normalization"
        ));
        assert!(trace_has_dependency(
            &trace,
            "extension_provider_module_ids",
            "parse_ok_module_ids"
        ));
        assert!(trace_has_dependency(
            &trace,
            "extension_provider_discovery_index",
            "parse_ok_module_ids"
        ));
        assert!(trace_has_dependency(
            &trace,
            "extension_provider_discovery_index",
            "extension_provider_summary"
        ));
        assert!(trace_has_dependency(
            &trace,
            "extension_provider_module_ids",
            "extension_provider_module_eligibility"
        ));
        assert!(!trace_has_dependency(
            &trace,
            "extension_provider_summary",
            "signature_item_tree"
        ));
        assert!(trace_has_dependency(
            &trace,
            "extension_signature_module_input",
            "module_defs"
        ));
        assert!(trace_has_dependency(
            &trace,
            "extension_signature_module_input",
            "signature_type_lowering"
        ));
        assert!(!trace_has_dependency(
            &trace,
            "program_trait_solving_signatures",
            "program_signature_module_ids"
        ));
        assert!(!trace_has_dependency(
            &trace,
            "program_trait_solving_signatures",
            "module_program_signature_facts"
        ));
        for query in [
            "extension_provider_validation_facts",
            "extension_trait_signature_index",
            "extension_provider_module_eligibility",
            "extension_provider_summary",
            "extension_signature_module_input",
            "extension_trait_solving_module_facts",
        ] {
            assert!(!trace.dependencies.iter().any(|dependency| {
                dependency.from.name == query
                    && matches!(
                        dependency.to.name,
                        "item_signatures" | "declaration_type_lowering"
                    )
            }));
            assert!(!trace.dependencies.iter().any(|dependency| {
                dependency.from.name == query && dependency.to.name == "active_module_item_tree"
            }));
            assert!(!trace.dependencies.iter().any(|dependency| {
                dependency.from.name == query && dependency.to.name == "full_module_defs"
            }));
            assert!(!trace.dependencies.iter().any(|dependency| {
                dependency.from.name == query && dependency.to.name == "program_type_normalizations"
            }));
        }
        {
            let query = "extension_method_index";
            assert!(trace.dependencies.iter().any(|dependency| {
                dependency.from.name == query
                    && dependency.to.name == "extension_provider_module_facts"
            }));
            assert!(!trace.dependencies.iter().any(|dependency| {
                dependency.from.name == query
                    && matches!(
                        dependency.to.name,
                        "signature_item_signatures"
                            | "signature_type_lowering"
                            | "signature_type_normalization"
                    )
            }));
        }
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_provider_module_facts"
                && dependency.to.name == "module_defs"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_provider_module_facts"
                && dependency.to.name == "signature_item_signatures"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_provider_module_facts"
                && dependency.to.name == "signature_type_lowering"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_provider_module_facts"
                && dependency.to.name == "signature_type_normalization"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_method_index"
                && dependency.to.name == "extension_provider_module_facts"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_method_index"
                && matches!(
                    dependency.to.name,
                    "item_signatures" | "declaration_type_lowering"
                )
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_method_index"
                && matches!(
                    dependency.to.name,
                    "extension_provider_validation_facts" | "program_trait_solving_signatures"
                )
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "declaration_type_lowering"
                && dependency.to.name == "program_defs_by_id"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "declaration_type_lowering"
                && dependency.to.name == "program_full_defs_by_id"
        }));
    }

    #[test]
    fn extension_provider_module_facts_refresh_across_source_revisions() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "struct S { value: i32 } extend S { pub fn make(value: i32) S { { value: value } } }",
        );
        let module_id = fixture.entry_id();
        let database = fixture.database();

        let _ = database.db.expect_get(ExtensionMethodIndexQuery);
        let before_update = database.query_trace();
        assert!(
            query_executions(&before_update, "extension_provider_module_facts") > 0,
            "{before_update:?}"
        );

        fixture.update_module_source(
            module_id,
            "struct S { value: i32 } extend S { pub fn make(value: i32) S { let next = value; { value: next } } }",
            SourceRevision(1),
        );
        database.update(CompileRequest::new(fixture.program()));
        let before_second_query = database.query_trace();

        let _ = database.db.expect_get(ExtensionMethodIndexQuery);
        let after_second_query = database.query_trace();

        assert_query_executions_unchanged(
            &before_second_query,
            &after_second_query,
            "extension_provider_summary",
        );
        assert!(
            query_executions(&before_second_query, "extension_provider_module_facts")
                < query_executions(&after_second_query, "extension_provider_module_facts")
        );
    }

    #[test]
    fn provider_summary_changes_validate_stable_module_eligibility() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "struct S {} extend S { pub fn first() i32 { 1 } }",
        );
        let module_id = fixture.entry_id();
        let database = fixture.database();
        let first = database
            .db
            .expect_get(ExtensionProviderModuleEligibilityQuery(module_id));
        assert!(*first);
        let first_modules = database.db.expect_get(ExtensionProviderModuleIdsQuery);

        fixture.update_module_source(
            module_id,
            "struct S {} extend S { pub fn first() i32 { 1 } pub fn second() i32 { 2 } }",
            SourceRevision(1),
        );
        database.update(CompileRequest::new(fixture.program()));

        let second = database
            .db
            .expect_get(ExtensionProviderModuleEligibilityQuery(module_id));
        assert!(*second);
        assert!(!Arc::ptr_eq(&first, &second));
        let latest_modules = database.db.expect_get(ExtensionProviderModuleIdsQuery);
        assert!(Arc::ptr_eq(&first_modules, &latest_modules));
        let trace = database.query_trace();
        let eligibility = trace
            .queries
            .iter()
            .find(|query| query.frame.name == "extension_provider_module_eligibility")
            .expect("extension provider eligibility trace");
        assert_eq!(eligibility.stats.executions, 2);
        assert_eq!(eligibility.stats.validations, 1);
        assert_eq!(eligibility.stats.green_validations, 0);
        let module_ids = trace
            .queries
            .iter()
            .find(|query| query.frame.name == "extension_provider_module_ids")
            .expect("extension provider module ids trace");
        assert_eq!(module_ids.stats.executions, 1);
        assert_eq!(module_ids.stats.validations, 1);
        assert_eq!(module_ids.stats.green_validations, 1);
    }

    #[test]
    fn body_sensitive_resolution_uses_full_active_item_tree_query() {
        let fixture =
            LoadedProgramFixture::new("main.nia", "fn main() i32 { let value = 1; value }");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let _ = db.expect_get(ValueResolutionQuery(module_id));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "value_resolution"
                && dependency.to.name == "full_active_module_item_tree"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "full_active_module_item_tree"
                && dependency.to.name == "full_module_item_tree"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "full_module_item_tree"
                && dependency.to.name == "full_module_item_tree_input"
        }));
    }

    #[test]
    fn value_resolution_does_not_build_visible_extensions_for_plain_paths() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module helper;

fn main() i32 {
    helper::value()
}
"#,
        );
        let entry_id = fixture.entry_id();
        fixture.add_child(entry_id, "helper", "helper.nia", "pub fn value() i32 { 1 }");
        let db = query_db(fixture.program());

        let values = db.expect_get(ValueResolutionQuery(entry_id));
        let trace = db.query_trace();

        assert!(values.diagnostics.is_empty(), "{:?}", values.diagnostics);
        assert!(!trace_has_dependency(
            &trace,
            "value_resolution",
            "visible_extensions"
        ));
        assert!(!trace_has_dependency(
            &trace,
            "value_resolution",
            "extension_provider_nominal_modules"
        ));
    }

    #[test]
    fn value_resolution_loads_visible_extensions_for_associated_values() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
struct S {}

extend S {
    const WIDTH: usize = 4usize;
}

fn main() usize {
    S::WIDTH
}
"#,
        );
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let values = db.expect_get(ValueResolutionQuery(module_id));
        let trace = db.query_trace();

        assert!(values.diagnostics.is_empty(), "{:?}", values.diagnostics);
        assert!(trace_has_dependency(
            &trace,
            "value_resolution",
            "visible_extensions"
        ));
    }

    #[test]
    fn flow_check_uses_full_active_item_tree_query() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { return 1; }");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let _ = db.expect_get(FlowCheckQuery(module_id));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "flow_check"
                && dependency.to.name == "full_active_module_item_tree"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "flow_check"
                && dependency.to.name == "signature_item_signatures"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "flow_check"
                && matches!(dependency.to.name, "item_signatures" | "type_lowering")
        }));
    }

    #[test]
    fn static_check_uses_full_active_item_tree_query() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "static mut global: i32 = 1; fn main() i32 { global }",
        );
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let _ = db.expect_get(StaticCheckQuery(module_id));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "static_check"
                && dependency.to.name == "full_active_module_item_tree"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "static_check"
                && dependency.to.name == "signature_item_signatures"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "static_check" && dependency.to.name == "const_values"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "static_check"
                && matches!(dependency.to.name, "item_signatures" | "const")
        }));
        assert!(
            !trace
                .dependencies
                .iter()
                .any(|dependency| dependency.from.name == "static_check"
                    && dependency.to.name == "program_const")
        );
        assert!(
            !trace
                .dependencies
                .iter()
                .any(|dependency| dependency.from.name == "static_check"
                    && dependency.to.name == "program_full_defs_by_id")
        );
    }

    #[test]
    fn body_check_collects_local_signature_subsets_with_full_type_lowering() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "struct S { value: i32 } static mut global: i32 = 1; fn main() i32 { global }",
        );
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let _ = db.expect_get(BodyCheckQuery(module_id));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "type_lowering"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "signature_item_tree"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check"
                && dependency.to.name == "signature_item_tree"
                && dependency.to.description.contains("ExtensionFunctions")
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "const_values"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "const_array_lengths"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check"
                && dependency.to.name == "program_body_function_signatures"
        }));
        for query in [
            "program_body_function_signatures",
            "program_body_value_signatures",
            "program_body_type_signatures",
            "program_body_trait_signatures",
        ] {
            assert!(
                !trace_has_dependency(&trace, "body_check", query),
                "body_check should not use {query}"
            );
        }
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check"
                && matches!(dependency.to.name, "item_signatures" | "const")
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check"
                && dependency.to.name == "signature_item_signatures"
                && dependency.to.description.contains("ExtensionFunctions")
        }));
    }

    #[test]
    fn body_check_reads_full_lowering_types_from_canonical_store() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
struct Item {
    state: i32,
}

fn set(items: &mut [Item], index: usize, state: i32) void {
    items[index].state = state;
}

fn main() i32 {
    let mut items: [2]Item = [
        { state: 1 },
        { state: 2 },
    ];
    set(&mut items[..], 1usize, 9);
    items[1].state
}
"#,
        );
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let checked = db.expect_get(BodyCheckQuery(module_id));

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }

    #[test]
    fn visible_extensions_use_signature_type_normalization_and_nominal_provider_queries() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module facade;
using entry::facade;

fn main(value: facade::Used) i32 {
    value.len()
}
"#,
        );
        let entry_id = fixture.entry_id();
        let facade_id = fixture.add_child(
            entry_id,
            "facade",
            "facade.nia",
            r#"
module impls;
module types;

pub using self::types::Used;
"#,
        );
        let impls_id = fixture.add_child_with_visibility(
            facade_id,
            "impls",
            nia_ids::Visibility::Private,
            "facade/impls.nia",
            r#"
using entry::facade::types::Used;

extend Used {
    pub fn len(&self) i32 {
        1
    }
}
"#,
        );
        fixture.add_child(facade_id, "types", "facade/types.nia", "pub struct Used {}");
        let impls_description = format!("{impls_id:?}");
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let _ = db.expect_get(VisibleExtensionsQuery(entry_id));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "visible_extensions"
                && dependency.to.name == "signature_type_normalization"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "visible_extensions"
                && dependency.to.name == "extension_provider_nominal_modules_for_targets"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_provider_nominal_modules_for_targets"
                && dependency.to.name == "extension_provider_nominal_target_names"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_provider_nominal_modules_for_targets"
                && dependency.to.name == "type_exposure_index"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_provider_nominal_modules_for_targets"
                && dependency.to.name == "extension_provider_nominal_candidate_modules"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.to.name == "extension_provider_nominal_candidate_index"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_provider_nominal_modules_for_targets"
                && dependency.to.name == "extension_provider_nominal_module_facts"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.to.name == "extension_provider_nominal_conservative_target_index"
        }));
        assert!(
            !trace
                .dependencies
                .iter()
                .any(|dependency| dependency.to.name == "extension_provider_nominal_index")
        );
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "visible_extensions"
                && dependency.to.name == "extension_provider_nominal_modules"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_provider_nominal_modules"
                && dependency.to.name == "extension_provider_module_facts"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "visible_extensions"
                && dependency.to.name == "extension_provider_module_facts"
                && dependency.to.description.contains(&impls_description)
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "visible_extensions"
                && dependency.to.name == "extension_method_index"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "visible_extensions"
                && dependency.to.name == "program_defs_by_id"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "visible_extensions" && dependency.to.name == "module_defs"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "visible_extensions"
                && dependency.to.name == "program_full_defs_by_id"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "visible_extensions"
                && dependency.to.name == "program_type_normalizations"
        }));
        assert!(
            !trace
                .dependencies
                .iter()
                .any(|dependency| dependency.to.name == "program_visible_type_signatures")
        );
        assert!(!depends_on_body_signature_query(
            &trace,
            "visible_extensions"
        ));
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
    fn executable_checked_program_uses_query_backed_extension_method_lookup() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "trait Show { fn show(self) i32; } extend i32 : Show { fn show(self) i32 { self } } pub fn main() i32 { 1.show() }",
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let checked = db.expect_get(CodegenProgramQuery);
        let trace = db.query_trace();

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_checked_module_facts"
                && dependency.to.name == "extension_trait_impls_for_trait"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_checked_module_facts"
                && dependency.to.name == "program_trait_solving_signatures"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_checked_module_facts"
                && dependency.to.name == "extension_provider_module_facts"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_checked_module_facts"
                && dependency.to.name == "extension_method_index"
        }));
        assert!(trace_has_dependency(
            &trace,
            "executable_checked_modules",
            "executable_checked_module_facts"
        ));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "checked_program"
                && dependency.to.name == "extension_provider_validation_facts"
        }));
    }

    #[test]
    fn bare_entry_checked_program_uses_rooted_diagnostics_without_freestanding_start() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "extend ! { fn nope(self) void {} } pub fn main() i32 { 1 }",
        );
        let db = query_db(fixture.program());

        let checked = db.expect_get(EntryCheckedProgramQuery);
        let trace = db.query_trace();

        assert!(
            checked.diagnostics.iter().any(|diagnostic| diagnostic
                .diagnostic
                .summary
                .contains("extend target must be an extendable value type")),
            "{:?}",
            checked.diagnostics
        );
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "entry_checked_program"
                && dependency.to.name == "executable_checked_modules"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "entry_checked_program"
                && dependency.to.name == "extension_provider_validation_facts"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "entry_checked_program"
                && dependency.to.name == "extension_method_index"
        }));
    }

    #[test]
    fn freestanding_entry_checked_program_uses_executable_reachability() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "extend ! { fn nope(self) void {} } pub fn main() i32 { 1 }",
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let checked = db.expect_get(EntryCheckedProgramQuery);
        let trace = db.query_trace();

        assert!(
            checked.diagnostics.iter().any(|diagnostic| diagnostic
                .diagnostic
                .summary
                .contains("extend target must be an extendable value type")),
            "{:?}",
            checked.diagnostics
        );
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "entry_checked_program"
                && dependency.to.name == "executable_checked_modules"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "entry_checked_program"
                && dependency.to.name == "extension_provider_validation_facts"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "entry_checked_program"
                && dependency.to.name == "extension_method_index"
        }));
    }

    #[test]
    fn executable_visible_extensions_follow_facade_provider_chains() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module facade;
using entry::facade;

fn main() i32 {
    let init = facade::Init::init();
    let args = init.args();
    let mut iter = args.iter();
    if ?value = iter.next() {
        value
    } or null {
        0
    }
}
"#,
        );
        let entry_id = fixture.entry_id();
        let facade_id = fixture.add_child(
            entry_id,
            "facade",
            "facade.nia",
            r#"
module args_impl;
module init_impl;
module types;

pub using self::types::{Args, ArgsIter, Init};
"#,
        );
        fixture.add_child_with_visibility(
            facade_id,
            "args_impl",
            nia_ids::Visibility::Private,
            "facade/args_impl.nia",
            r#"
using entry::facade::types::{Args, ArgsIter};

extend Args {
    pub fn iter(&self) ArgsIter {
        ArgsIter {}
    }
}

extend ArgsIter {
    pub fn next(&mut self) ?i32 {
        ?42
    }
}
"#,
        );
        fixture.add_child_with_visibility(
            facade_id,
            "init_impl",
            nia_ids::Visibility::Private,
            "facade/init_impl.nia",
            r#"
using entry::facade::types::{Args, Init};

extend Init {
    pub fn init() Init {
        {}
    }

    pub fn args(&self) Args {
        Args {}
    }
}
"#,
        );
        fixture.add_child(
            facade_id,
            "types",
            "facade/types.nia",
            r#"
pub struct Init {}
pub struct Args {}
pub struct ArgsIter {}
"#,
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let checked = db.expect_get(CodegenProgramQuery);

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }

    #[test]
    fn visible_extensions_do_not_expand_using_type_modules_as_provider_modules() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module facade;
using entry::facade;

fn main(value: facade::Used) i32 {
    value.len()
}
"#,
        );
        let entry_id = fixture.entry_id();
        let facade_id = fixture.add_child(
            entry_id,
            "facade",
            "facade.nia",
            r#"
module impls;
module types;

pub using self::types::{Unused, Used};
"#,
        );
        fixture.add_child_with_visibility(
            facade_id,
            "impls",
            nia_ids::Visibility::Private,
            "facade/impls.nia",
            r#"
using entry::facade::types::Used;

extend Used {
    pub fn len(&self) i32 {
        1
    }
}
"#,
        );
        let types_id = fixture.add_child(
            facade_id,
            "types",
            "facade/types.nia",
            r#"
pub struct Unused {}
pub struct Used {}
"#,
        );
        let entry_description = format!("{entry_id:?}");
        let types_description = format!("{types_id:?}");
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let checked = db.expect_get(CodegenProgramQuery);

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let trace = db.query_trace();
        assert!(
            !trace.dependencies.iter().any(|dependency| {
                dependency.from.name == "visible_extensions"
                    && dependency.from.description.contains(&entry_description)
                    && dependency.to.description.contains(&types_description)
                    && dependency.to.name == "signature_type_normalization"
            }),
            "visible extensions should not normalize every module that merely defines a using-imported type"
        );
    }

    #[test]
    fn visible_trait_impls_follow_facade_reexport_item_modules() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module fmt;
using entry::fmt;

fn main() i32 {
    fmt::parse[i32](&"abc")
}
"#,
        );
        let entry_id = fixture.entry_id();
        let fmt_id = fixture.add_child(
            entry_id,
            "fmt",
            "fmt.nia",
            r#"
pub module parse_impl;
pub using parse_impl::{ParseFrom, parse};
"#,
        );
        let parse_impl_id = fixture.add_child(
            fmt_id,
            "parse_impl",
            "fmt/parse_impl.nia",
            r#"
pub trait ParseFrom[Input] {
    fn parse_from(input: Input) Self;
}

pub fn parse[T, Input](input: Input) T
where T: ParseFrom[Input]
{
    [T]::parse_from(input)
}

extend i32 : ParseFrom[&[char]] {
    fn parse_from(input: &[char]) i32 {
        input.len() as i32
    }
}

extend i32 : ParseFrom[&[u8]] {
    fn parse_from(input: &[u8]) i32 {
        input.len() as i32
    }
}
"#,
        );
        let db = query_db(fixture.program());

        let trait_impls = db.expect_get(VisibleTraitImplsQuery(entry_id));

        assert_eq!(trait_impls.trait_impls.len(), 2);
        assert!(
            trait_impls
                .trait_impls
                .iter()
                .all(|impl_signature| impl_signature.module_id == parse_impl_id),
            "{:?}",
            trait_impls.trait_impls
        );
    }

    #[test]
    fn executable_reachability_keeps_matched_trait_impl_method_bodies() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module parse;
using entry::parse;

pub fn main() i32 {
    parse::parse[i32, parse::Input](parse::Input {})
}
"#,
        );
        let entry_id = fixture.entry_id();
        let parse_id = fixture.add_child(
            entry_id,
            "parse",
            "parse.nia",
            r#"
pub struct Input {}

pub trait ParseFrom[Input] {
    fn parse_from(input: Input) Self;
}

pub fn parse[T, Input](input: Input) T
where T: ParseFrom[Input]
{
    [T]::parse_from(input)
}

extend i32 : ParseFrom[Input] {
    fn parse_from(input: Input) i32 {
        _ = input;
        42
    }
}
"#,
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let checked = db.expect_get(ExecutableCheckedModulesQuery);
        let parse_module = checked
            .iter()
            .find(|module| module.id == parse_id)
            .expect("parse module should be executable-reachable");
        let parse_from = parse_module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == sym("parse_from") && def.kind == nia_defs::DefKind::Method).then_some(
                    GlobalDefId {
                        module_id: parse_id,
                        def_id,
                    },
                )
            })
            .expect("impl parse_from method should be defined");

        assert!(
            parse_module
                .body_ir
                .function_bodies
                .contains_key(&parse_from),
            "matched trait impl method body should be retained for executable codegen"
        );
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
    fn signature_type_resolution_separates_semantic_value_from_diagnostics() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main(value: Missing) {}");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let resolution = db.expect_get(SignatureTypeResolutionQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ));
        assert!(resolution.semantic.diagnostics.is_empty());
        assert!(!resolve_diagnostic_bundle(db.context(), &resolution.diagnostics).is_empty());
    }

    #[test]
    fn type_resolution_separates_semantic_value_from_diagnostics() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main(value: Missing) {}");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let resolution = db.expect_get(TypeResolutionQuery(module_id));
        assert!(resolution.semantic.diagnostics.is_empty());
        assert!(!resolve_diagnostic_bundle(db.context(), &resolution.diagnostics).is_empty());
    }

    #[test]
    fn signature_type_lowering_separates_semantic_value_from_diagnostics() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "struct Box[T] { value: T } fn main(value: Box) {}",
        );
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let lowering = db.expect_get(SignatureTypeLoweringQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ));
        assert!(lowering.semantic.diagnostics.is_empty());
        assert!(
            resolve_diagnostic_bundle(db.context(), &lowering.diagnostics)
                .iter()
                .any(|diagnostic| diagnostic
                    .summary
                    .contains("generic argument count mismatch"))
        );
    }

    #[test]
    fn type_lowering_separates_semantic_value_from_diagnostics() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "struct Box[T] { value: T } fn main(value: Box) {}",
        );
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let lowering = db.expect_get(TypeLoweringQuery(module_id));
        assert!(lowering.semantic.diagnostics.is_empty());
        assert!(
            resolve_diagnostic_bundle(db.context(), &lowering.diagnostics)
                .iter()
                .any(|diagnostic| diagnostic
                    .summary
                    .contains("generic argument count mismatch"))
        );
    }

    #[test]
    fn signature_item_signatures_separate_semantic_value_from_diagnostics() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn missing_body() void;");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let signatures = db.expect_get(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ));
        assert!(signatures.semantic.diagnostics.is_empty());
        assert!(
            resolve_diagnostic_bundle(db.context(), &signatures.diagnostics)
                .iter()
                .any(|diagnostic| diagnostic
                    .summary
                    .contains("bodyless non-extern functions require `@[builtin]`"))
        );
    }

    #[test]
    fn item_signatures_separate_semantic_value_from_diagnostics() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn missing_body() void;");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let signatures = db.expect_get(ItemSignaturesQuery(module_id));
        assert!(signatures.semantic.diagnostics.is_empty());
        assert!(
            resolve_diagnostic_bundle(db.context(), &signatures.diagnostics)
                .iter()
                .any(|diagnostic| diagnostic
                    .summary
                    .contains("bodyless non-extern functions require `@[builtin]`"))
        );
    }

    #[test]
    fn value_resolution_separates_semantic_value_from_diagnostics() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "module helper; fn main() i32 { helper::missing() }",
        );
        let module_id = fixture.entry_id();
        fixture.add_child(
            module_id,
            "helper",
            "helper.nia",
            "pub fn value() i32 { 1 }",
        );
        let db = query_db(fixture.program());

        let resolution = db.expect_get(ValueResolutionQuery(module_id));
        assert!(resolution.semantic.diagnostics.is_empty());
        assert!(!resolve_diagnostic_bundle(db.context(), &resolution.diagnostics).is_empty());
    }

    #[test]
    fn local_resolution_separates_semantic_value_from_diagnostics() {
        let fixture =
            LoadedProgramFixture::new("main.nia", "fn main(value: i32, value: i32) i32 { value }");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let resolution = db.expect_get(LocalResolutionQuery(module_id));
        assert!(resolution.semantic.diagnostics.is_empty());
        assert!(!resolve_diagnostic_bundle(db.context(), &resolution.diagnostics).is_empty());
    }

    #[test]
    fn flow_check_separates_semantic_value_from_diagnostics() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "fn main(flag: bool) i32 { if flag { return 1; } }",
        );
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let flow_check = db.expect_get(FlowCheckQuery(module_id));
        assert!(flow_check.semantic.diagnostics.is_empty());
        assert!(
            resolve_diagnostic_bundle(db.context(), &flow_check.diagnostics)
                .iter()
                .any(|diagnostic| diagnostic
                    .summary
                    .contains("does not return on all reachable paths"))
        );
    }

    #[test]
    fn terminal_checks_separate_semantic_values_from_diagnostics() {
        let static_fixture = LoadedProgramFixture::new(
            "main.nia",
            "static global: i32 = make(); fn make() i32 { 1 }",
        );
        let static_module_id = static_fixture.entry_id();
        let static_db = query_db(static_fixture.program());
        let static_check = static_db.expect_get(StaticCheckQuery(static_module_id));

        assert!(static_check.semantic.diagnostics.is_empty());
        assert!(
            resolve_diagnostic_bundle(static_db.context(), &static_check.diagnostics)
                .iter()
                .any(|diagnostic| diagnostic
                    .summary
                    .contains("global initializer is not static data"))
        );

        let abi_fixture = LoadedProgramFixture::new("main.nia", "extern fn bad(flag: bool) void;");
        let abi_module_id = abi_fixture.entry_id();
        let abi_db = query_db(abi_fixture.program());
        let abi_check = abi_db.expect_get(AbiCheckQuery(abi_module_id));

        assert!(abi_check.semantic.diagnostics.is_empty());
        assert!(
            resolve_diagnostic_bundle(abi_db.context(), &abi_check.diagnostics)
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("cannot use `bool` directly"))
        );
    }

    #[test]
    fn layouts_separate_semantic_value_from_diagnostics() {
        let fixture = LoadedProgramFixture::new("main.nia", "struct Node { next: Node }");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let layouts = db.expect_get(LayoutsQuery(module_id));
        assert!(layouts.semantic.diagnostics.is_empty());
        assert!(
            resolve_diagnostic_bundle(db.context(), &layouts.diagnostics)
                .iter()
                .any(|diagnostic| diagnostic
                    .summary
                    .contains("recursive struct layout is not supported"))
        );
    }

    #[test]
    fn const_check_separates_semantic_value_from_diagnostics() {
        let fixture = LoadedProgramFixture::new("main.nia", "const a: i32 = b; const b: i32 = a;");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let const_eval = db.expect_get(ConstQuery(module_id));
        assert!(const_eval.semantic.diagnostics.is_empty());
        assert!(!resolve_diagnostic_bundle(db.context(), &const_eval.diagnostics).is_empty());
    }

    #[test]
    fn monomorphization_separates_semantic_value_from_diagnostics() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "fn grow[T](value: &T) i32 { grow[&T](&value) } fn main() i32 { let value: i32 = 1; grow[i32](&value) }",
        );
        let db = query_db(fixture.program());

        let monomorphization = db.expect_get(MonomorphizationQuery);
        assert!(monomorphization.semantic.diagnostics.is_empty());
        assert!(
            resolve_diagnostic_bundle(db.context(), &monomorphization.diagnostics)
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("type depth limit"))
        );
    }

    #[test]
    fn body_check_separates_semantic_value_from_diagnostics() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { false }");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let body_check = db.expect_get(BodyCheckQuery(module_id));
        assert!(body_check.semantic.diagnostics.is_empty());
        assert!(!resolve_diagnostic_bundle(db.context(), &body_check.diagnostics).is_empty());
    }

    #[test]
    fn signature_type_normalization_separates_semantic_value_from_diagnostics() {
        let fixture = LoadedProgramFixture::new("main.nia", "type A = B; type B = A;");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let normalization = db.expect_get(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Types,
        ));
        assert!(normalization.semantic.diagnostics.is_empty());
        assert!(!resolve_diagnostic_bundle(db.context(), &normalization.diagnostics).is_empty());
    }

    #[test]
    fn type_normalization_separates_semantic_value_from_diagnostics() {
        let fixture = LoadedProgramFixture::new("main.nia", "type A = B; type B = A;");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let normalization = db.expect_get(TypeNormalizationQuery(module_id));
        assert!(normalization.semantic.diagnostics.is_empty());
        assert!(!resolve_diagnostic_bundle(db.context(), &normalization.diagnostics).is_empty());
    }

    #[test]
    fn checked_module_exposes_semantic_use_table_product() {
        let source = "fn main() i32 { let mut local: i32 = 1; local }";
        let fixture = LoadedProgramFixture::new("main.nia", source);
        let checked = fixture.database().analyze_program();

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let module = checked.modules.first().expect("checked module");
        let function_facts = module
            .semantic_facts
            .function_facts
            .values()
            .next()
            .expect("function semantic facts");
        assert_eq!(
            function_facts.store_id(),
            module.semantic_uses.store_id(),
            "frozen body facts should share the compiler session node owner"
        );
        assert_eq!(
            module.semantic_facts.store_id(),
            module.semantic_uses.store_id(),
            "module semantic facts should share the compiler session node owner"
        );
        assert!(matches!(
            module
                .semantic_uses
                .node_value_uses
                .values()
                .find(|value_use| matches!(value_use, SemanticValueUse::Local(_))),
            Some(SemanticValueUse::Local(_))
        ));
    }

    #[test]
    fn checked_module_reuses_cached_semantic_product_handles() {
        let fixture =
            LoadedProgramFixture::new("main.nia", "fn main() i32 { let local: i32 = 1; local }");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let checked = db.expect_get(CheckedModuleQuery(module_id));
        let checked_program = db.expect_get(CheckedProgramQuery);
        let values = db.expect_get(ValueResolutionQuery(module_id));
        let locals = db.expect_get(LocalResolutionQuery(module_id));
        let semantic_uses = db.expect_get(SemanticUseTableQuery(module_id));
        let type_resolution = db.expect_get(TypeResolutionQuery(module_id));
        let type_lowering = db.expect_get(TypeLoweringQuery(module_id));
        let type_normalization = db.expect_get(TypeNormalizationQuery(module_id));
        let layouts = db.expect_get(LayoutsQuery(module_id));
        let body_check = db.expect_get(BodyCheckQuery(module_id));
        let const_eval = db.expect_get(ConstQuery(module_id));
        let const_array_lengths = db.expect_get(ConstArrayLengthsQuery(module_id));
        let const_enum_values = db.expect_get(ConstEnumValuesQuery(module_id));
        let const_values = db.expect_get(ConstValuesQuery(module_id));
        let const_typed_facts = db.expect_get(ConstTypedFactsQuery(module_id));
        let static_check = db.expect_get(StaticCheckQuery(module_id));
        let abi_check = db.expect_get(AbiCheckQuery(module_id));
        let flow_check = db.expect_get(FlowCheckQuery(module_id));

        assert!(Arc::ptr_eq(&checked, &checked_program.modules[0]));
        assert!(Arc::ptr_eq(&checked.value_resolution, &values.semantic));
        assert!(Arc::ptr_eq(&checked.local_resolution, &locals.semantic));
        assert!(Arc::ptr_eq(&checked.semantic_uses, &semantic_uses));
        assert!(Arc::ptr_eq(
            &checked.type_resolution,
            &type_resolution.semantic
        ));
        assert!(Arc::ptr_eq(&checked.type_lowering, &type_lowering.semantic));
        assert!(Arc::ptr_eq(
            &checked.type_normalization,
            &type_normalization.semantic
        ));
        assert!(Arc::ptr_eq(&checked.layouts, &layouts.semantic));
        assert!(Arc::ptr_eq(&checked.body_ir, &body_check.semantic.ir));
        assert!(Arc::ptr_eq(
            &checked.semantic_facts,
            &body_check.semantic.facts
        ));
        assert!(Arc::ptr_eq(
            &checked.provider_demands,
            &body_check.semantic.provider_demands
        ));
        assert_eq!(checked.body_diagnostics, body_check.diagnostics);
        assert_eq!(checked.const_diagnostics, const_eval.diagnostics);
        assert!(Arc::ptr_eq(&checked.const_eval, &const_eval.semantic));
        assert!(Arc::ptr_eq(
            &const_eval.semantic.values,
            &const_values.values
        ));
        assert!(Arc::ptr_eq(
            &const_eval.semantic.typed_values,
            &const_typed_facts.typed_values
        ));
        assert!(Arc::ptr_eq(
            &const_eval.semantic.enum_values,
            &const_enum_values.values
        ));
        assert!(Arc::ptr_eq(
            &const_eval.semantic.typed_enum_values,
            &const_enum_values.typed_values
        ));
        assert!(Arc::ptr_eq(
            &const_eval.semantic.array_lengths,
            &const_array_lengths.values
        ));
        assert!(Arc::ptr_eq(&checked.static_check, &static_check.semantic));
        assert!(Arc::ptr_eq(&checked.abi_check, &abi_check.semantic));
        assert!(Arc::ptr_eq(&checked.flow_check, &flow_check.semantic));
        assert_eq!(checked.static_diagnostics, static_check.diagnostics);
        assert_eq!(checked.layout_diagnostics, layouts.diagnostics);
        assert_eq!(checked.abi_diagnostics, abi_check.diagnostics);
        assert_eq!(checked.flow_diagnostics, flow_check.diagnostics);
    }

    #[test]
    fn program_products_share_the_input_module_graph_snapshot() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let loaded = fixture.program();
        let input_graph = loaded.graph.clone();
        let db = query_db(loaded);

        let cached_graph = db.expect_get(ModuleGraphQuery);
        let checked = db.expect_get(CheckedProgramQuery);
        let codegen = db.expect_get(CodegenProgramQuery);

        assert!(input_graph.ptr_eq(&cached_graph));
        assert!(input_graph.ptr_eq(&checked.graph));
        assert!(input_graph.ptr_eq(&codegen.graph));
    }

    #[test]
    fn checked_modules_reuse_cached_definition_handles() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 1 }");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let defs = db.expect_get(FullModuleDefsQuery(module_id));
        let checked = db.expect_get(CheckedModuleQuery(module_id));
        let executable = db.expect_get(ExecutableCheckedModulesQuery);
        let executable = executable
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry executable module");

        assert!(Arc::ptr_eq(&checked.defs, &defs.semantic));
        assert!(Arc::ptr_eq(&executable.defs, &defs.semantic));
        assert_eq!(checked.definition_diagnostics, defs.diagnostics);
        assert_eq!(executable.definition_diagnostics, defs.diagnostics);
    }

    #[test]
    fn full_module_definitions_separate_semantic_value_from_diagnostics() {
        let fixture =
            LoadedProgramFixture::new("main.nia", "struct Duplicate {} struct Duplicate {}");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let definitions = db.expect_get(FullModuleDefsQuery(module_id));
        let checked = db.expect_get(CheckedModuleQuery(module_id));

        assert!(definitions.semantic.diagnostics.is_empty());
        assert!(
            resolve_diagnostic_bundle(db.context(), &definitions.diagnostics)
                .iter()
                .any(|diagnostic| diagnostic
                    .primary_message()
                    .is_some_and(|message| message.contains("duplicate type definition")))
        );
        assert!(Arc::ptr_eq(&checked.defs, &definitions.semantic));
        assert_eq!(checked.definition_diagnostics, definitions.diagnostics);
    }

    #[test]
    fn compiler_fact_batches_reuse_cached_product_handles() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "struct S { value: i32 } extend S { fn get(self) i32 { self.value } }",
        );
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());
        let signature_set = nia_item_tree::SignatureItemSet::Types;

        let signature = db.expect_get(ModuleProgramSignatureFactsQuery(module_id, signature_set));
        let abi = db.expect_get(ModuleAbiSignatureFactsQuery(module_id));
        let trait_solving = db.expect_get(ExtensionTraitSolvingModuleFactsQuery(module_id));
        let provider = db.expect_get(ExtensionProviderModuleFactsQuery(module_id));
        let nominal = db.expect_get(ExtensionProviderNominalModuleFactsQuery(module_id));
        let visible_extensions: Arc<VisibleExtensionsForModule> =
            db.expect_get(VisibleExtensionsQuery(module_id));
        let visible_trait_impls: Arc<VisibleTraitImplsForModule> =
            db.expect_get(VisibleTraitImplsQuery(module_id));
        let trait_method_index: Arc<nia_program_signatures::ProgramTraitMethodIndex> =
            db.expect_get(ProgramTraitMethodIndexQuery);
        let abi_signatures: Arc<ProgramAbiSignaturesValue> =
            db.expect_get(ProgramAbiSignaturesQuery);

        let signature_batch = db
            .get_many([ModuleProgramSignatureFactsQuery(module_id, signature_set)])
            .expect("signature batch should succeed");
        let abi_batch = db
            .get_many([ModuleAbiSignatureFactsQuery(module_id)])
            .expect("ABI batch should succeed");
        let trait_solving_batch = db
            .get_many([ExtensionTraitSolvingModuleFactsQuery(module_id)])
            .expect("trait-solving batch should succeed");
        let provider_batch = db
            .get_many([ExtensionProviderModuleFactsQuery(module_id)])
            .expect("provider batch should succeed");
        let nominal_batch = db
            .get_many([ExtensionProviderNominalModuleFactsQuery(module_id)])
            .expect("nominal provider batch should succeed");
        let visible_extensions_batch = db
            .get_many([VisibleExtensionsQuery(module_id)])
            .expect("visible extension batch should succeed");
        let visible_trait_impls_batch = db
            .get_many([VisibleTraitImplsQuery(module_id)])
            .expect("visible trait-impl batch should succeed");
        let trait_method_index_batch = db
            .get_many([ProgramTraitMethodIndexQuery])
            .expect("trait-method index batch should succeed");
        let abi_signatures_batch = db
            .get_many([ProgramAbiSignaturesQuery])
            .expect("program ABI batch should succeed");

        assert!(Arc::ptr_eq(&signature, &signature_batch[0]));
        assert!(Arc::ptr_eq(&abi, &abi_batch[0]));
        assert!(Arc::ptr_eq(&trait_solving, &trait_solving_batch[0]));
        assert!(Arc::ptr_eq(&provider, &provider_batch[0]));
        assert!(Arc::ptr_eq(&nominal, &nominal_batch[0]));
        assert!(Arc::ptr_eq(
            &visible_extensions,
            &visible_extensions_batch[0]
        ));
        assert!(Arc::ptr_eq(
            &visible_trait_impls,
            &visible_trait_impls_batch[0]
        ));
        assert!(Arc::ptr_eq(
            &trait_method_index,
            &trait_method_index_batch[0]
        ));
        assert!(Arc::ptr_eq(&abi_signatures, &abi_signatures_batch[0]));
    }

    #[test]
    fn extension_index_queries_reuse_single_layer_product_handles() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "trait Read { fn read(self) i32; } struct S { value: i32 } extend S { fn get(self) i32 { self.value } }",
        );
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());
        let defs = db.expect_get(ModuleDefsQuery(module_id));
        let trait_id = nia_ty::TraitId::Source(GlobalDefId {
            module_id,
            def_id: defs.semantic.module_scope.types.get(&sym("Read")).unwrap(),
        });
        let method_id = GlobalDefId {
            module_id,
            def_id: nia_ids::DefId(0),
        };

        let validation: Arc<ExtensionProviderValidationFactsQueryValue> =
            db.expect_get(ExtensionProviderValidationFactsQuery(module_id));
        let discovery: Arc<ExtensionProviderDiscoveryIndexQueryValue> =
            db.expect_get(ExtensionProviderDiscoveryIndexQuery);
        let exposure: Arc<TypeExposureIndex> = db.expect_get(TypeExposureIndexQuery);
        let methods: Arc<ExtensionMethodIndexQueryValue> = db.expect_get(ExtensionMethodIndexQuery);
        let named: Arc<ExtensionMethodsNamedQueryValue> =
            db.expect_get(ExtensionMethodsNamedQuery(sym("get")));
        let method: Arc<ExtensionMethodByIdQueryValue> =
            db.expect_get(ExtensionMethodByIdQuery(method_id));
        let trait_index: Arc<ExtensionTraitSignatureIndex> =
            db.expect_get(ExtensionTraitSignatureIndexQuery);
        let signature_input: Arc<ExtensionSignatureModuleInputQueryValue> =
            db.expect_get(ExtensionSignatureModuleInputQuery(module_id));
        let trait_impls: Arc<ExtensionTraitImplsForTraitQueryValue> =
            db.expect_get(ExtensionTraitImplsForTraitQuery(trait_id));

        let validation_batch = db
            .get_many([ExtensionProviderValidationFactsQuery(module_id)])
            .expect("validation batch should succeed");
        let discovery_batch = db
            .get_many([ExtensionProviderDiscoveryIndexQuery])
            .expect("discovery batch should succeed");
        let exposure_batch = db
            .get_many([TypeExposureIndexQuery])
            .expect("exposure batch should succeed");
        let methods_batch = db
            .get_many([ExtensionMethodIndexQuery])
            .expect("method index batch should succeed");
        let named_batch = db
            .get_many([ExtensionMethodsNamedQuery(sym("get"))])
            .expect("named method batch should succeed");
        let method_batch = db
            .get_many([ExtensionMethodByIdQuery(method_id)])
            .expect("method batch should succeed");
        let trait_index_batch = db
            .get_many([ExtensionTraitSignatureIndexQuery])
            .expect("trait signature batch should succeed");
        let signature_input_batch = db
            .get_many([ExtensionSignatureModuleInputQuery(module_id)])
            .expect("signature input batch should succeed");
        let trait_impls_batch = db
            .get_many([ExtensionTraitImplsForTraitQuery(trait_id)])
            .expect("trait impl batch should succeed");

        assert!(Arc::ptr_eq(&validation, &validation_batch[0]));
        assert!(Arc::ptr_eq(&discovery, &discovery_batch[0]));
        assert!(Arc::ptr_eq(&exposure, &exposure_batch[0]));
        assert!(Arc::ptr_eq(&methods, &methods_batch[0]));
        assert!(Arc::ptr_eq(&named, &named_batch[0]));
        assert!(Arc::ptr_eq(&method, &method_batch[0]));
        assert!(Arc::ptr_eq(&trait_index, &trait_index_batch[0]));
        assert!(Arc::ptr_eq(&signature_input, &signature_input_batch[0]));
        assert!(Arc::ptr_eq(&trait_impls, &trait_impls_batch[0]));
    }

    #[test]
    fn public_surface_queries_reuse_single_layer_product_handles() {
        let fixture = LoadedProgramFixture::new("main.nia", "pub struct S { value: i32 }");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let surfaces: Arc<PublicSurfacesQueryValue> = db.expect_get(PublicSurfacesQuery);
        let using_scopes: Arc<PublicUsingScopesQueryValue> = db.expect_get(PublicUsingScopesQuery);
        let module_using_scope: Arc<ModuleUsingScope> =
            db.expect_get(ModuleUsingScopeQuery(module_id));

        let surfaces_batch = db
            .get_many([PublicSurfacesQuery])
            .expect("public surfaces batch should succeed");
        let using_scopes_batch = db
            .get_many([PublicUsingScopesQuery])
            .expect("public using scopes batch should succeed");
        let module_using_scope_batch = db
            .get_many([ModuleUsingScopeQuery(module_id)])
            .expect("module using scope batch should succeed");

        assert!(Arc::ptr_eq(&surfaces, &surfaces_batch[0]));
        assert!(Arc::ptr_eq(&using_scopes, &using_scopes_batch[0]));
        assert!(Arc::ptr_eq(
            &module_using_scope,
            &module_using_scope_batch[0]
        ));
    }

    #[test]
    fn backend_lowering_uses_executable_per_item_ir() {
        let fixture =
            LoadedProgramFixture::new("main.nia", "fn main() i32 { static value: i32 = 1; value }");
        let db = query_db(fixture.program());

        let _ = db.expect_get(BackendLoweringQuery);
        let trace = db.query_trace();

        assert!(trace_has_dependency(
            &trace,
            "backend_lowering",
            "backend_item_plan"
        ));
        assert!(trace_has_dependency(
            &trace,
            "backend_item_plan",
            "backend_lowering_inputs"
        ));
        assert!(trace_has_dependency(
            &trace,
            "backend_lowering",
            "backend_module_finalization"
        ));
        assert!(trace_has_dependency(
            &trace,
            "backend_module_finalization",
            "backend_module_item_plan"
        ));
        assert!(trace_has_dependency(
            &trace,
            "backend_module_finalization",
            "backend_finalization_task_context"
        ));
        assert!(trace_has_dependency(
            &trace,
            "backend_finalization_task_context",
            "backend_lowering_inputs"
        ));
        assert!(trace_has_dependency(
            &trace,
            "backend_lowering_inputs",
            "backend_module_source_item_plan"
        ));
        assert!(trace_has_dependency(
            &trace,
            "backend_lowering_inputs",
            "backend_module_function_instance_plan"
        ));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering_inputs"
                && dependency.to.name == "executable_checked_modules"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering_inputs"
                && dependency.to.name == "full_active_module_item_tree"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering" && dependency.to.name == "checked_module_ids"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering_inputs"
                && dependency.to.name == "signature_item_tree"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering"
                && dependency.to.name == "program_full_defs_by_id"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering"
                && dependency.to.name == "program_backend_signatures"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering" && dependency.to.name == "const_enum_values"
        }));
        assert!(!depends_on_body_signature_query(&trace, "backend_lowering"));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering"
                && dependency.to.name == "program_type_normalizations"
        }));
    }

    #[test]
    fn codegen_tracks_and_reuses_backend_stage_products() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "module helper; using entry::helper; fn main() i32 { helper::id[i32](1) }",
        );
        let module_id = fixture.entry_id();
        let helper_id = fixture.add_child(
            module_id,
            "helper",
            "helper.nia",
            "pub fn id[T](value: T) T { value }",
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let first_codegen = db.expect_get(CodegenProgramQuery);
        assert!(
            first_codegen.diagnostics.is_empty(),
            "{:?}",
            first_codegen.diagnostics
        );
        let monomorphization = db.expect_get(MonomorphizationQuery);
        let entry_source_item_plan = db.expect_get(BackendModuleSourceItemPlanQuery(module_id));
        let helper_source_item_plan = db.expect_get(BackendModuleSourceItemPlanQuery(helper_id));
        let entry_function_instance_plan =
            db.expect_get(BackendModuleFunctionInstancePlanQuery(module_id));
        let helper_function_instance_plan =
            db.expect_get(BackendModuleFunctionInstancePlanQuery(helper_id));
        let backend_lowering = db.expect_get(BackendLoweringQuery);
        let second_codegen = db.expect_get(CodegenProgramQuery);
        let trace = db.query_trace();

        assert!(Arc::ptr_eq(&first_codegen, &second_codegen));
        assert!(Arc::ptr_eq(
            &first_codegen.monomorphization,
            &monomorphization.semantic
        ));
        assert!(Arc::ptr_eq(
            &first_codegen.backend_lowering,
            &backend_lowering.semantic
        ));
        assert!(trace_has_dependency(
            &trace,
            "codegen_program",
            "codegen_preparation"
        ));
        assert!(trace_has_dependency(
            &trace,
            "codegen_preparation",
            "monomorphization"
        ));
        assert!(trace_has_dependency(
            &trace,
            "codegen_program",
            "backend_lowering"
        ));
        assert!(trace_has_dependency(
            &trace,
            "backend_lowering",
            "backend_item_plan"
        ));
        assert!(trace_has_dependency(
            &trace,
            "backend_item_plan",
            "backend_lowering_inputs"
        ));
        assert!(trace_has_dependency(
            &trace,
            "backend_lowering_inputs",
            "backend_module_source_item_plan"
        ));
        assert!(trace_has_dependency(
            &trace,
            "backend_lowering_inputs",
            "backend_module_function_instance_plan"
        ));
        assert!(trace_has_dependency(
            &trace,
            "backend_module_function_instance_plan",
            "monomorphization"
        ));
        assert!(trace_has_dependency(
            &trace,
            "backend_lowering",
            "backend_module_finalization"
        ));
        assert!(!trace_has_dependency(
            &trace,
            "backend_lowering",
            "monomorphization"
        ));
        assert_eq!(entry_source_item_plan.functions.len(), 1);
        assert_eq!(helper_source_item_plan.functions.len(), 1);
        assert!(entry_function_instance_plan.instances.is_empty());
        assert_eq!(helper_function_instance_plan.instances.len(), 1);
        assert_eq!(
            helper_function_instance_plan.instances[0].def_id.module_id,
            helper_id
        );
        assert_eq!(
            helper_function_instance_plan.instances[0].arg_module_id,
            module_id
        );
        assert_eq!(query_executions(&trace, "codegen_program"), 1);
        assert_eq!(query_executions(&trace, "monomorphization"), 1);
        assert_eq!(query_executions(&trace, "backend_item_plan"), 1);
        assert_eq!(query_executions(&trace, "backend_module_item_plan"), 0);
        assert_eq!(query_executions(&trace, "backend_lowering_inputs"), 1);
        assert_eq!(
            query_executions(&trace, "backend_finalization_task_context"),
            1
        );
        assert_eq!(query_executions(&trace, "backend_module_finalization"), 2);
        assert_eq!(
            query_executions(&trace, "backend_module_source_item_plan"),
            2
        );
        assert_eq!(
            query_executions(&trace, "backend_module_function_instance_plan"),
            2
        );
        assert_eq!(query_executions(&trace, "backend_lowering"), 1);
    }

    #[test]
    fn backend_module_plan_slots_are_consumed_and_republished_after_invalidation() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "module helper; using entry::helper; fn main() i32 { helper::value() }",
        );
        let module_id = fixture.entry_id();
        let helper_id = fixture.add_child(
            module_id,
            "helper",
            "helper.nia",
            "pub fn value() i32 { 0 }",
        );
        let db = query_db(fixture.program());

        let first = db.expect_get(BackendLoweringQuery);
        assert!(first.semantic.diagnostics.is_empty());
        assert!(
            resolve_diagnostic_bundle(db.context(), &first.diagnostics).is_empty(),
            "{:?}",
            resolve_diagnostic_bundle(db.context(), &first.diagnostics)
        );
        assert_eq!(
            first
                .semantic
                .program
                .modules
                .iter()
                .map(|module| module.id)
                .collect::<Vec<_>>(),
            vec![module_id, helper_id]
        );
        for owner in [module_id, helper_id] {
            assert!(
                db.get_owned(BackendModuleItemPlanQuery(owner)).is_err(),
                "finalization must leave no module-plan payload in its query slot"
            );
        }

        db.invalidate(BackendLoweringQuery);
        let second = db.expect_get(BackendLoweringQuery);
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);

        let invalidation = db.invalidate(BackendItemPlanQuery);
        assert!(
            invalidation
                .invalidated
                .iter()
                .any(|frame| { frame.name == "backend_module_item_plan" })
        );
        assert!(
            invalidation
                .invalidated
                .iter()
                .any(|frame| { frame.name == "backend_lowering" })
        );

        let third = db.expect_get(BackendLoweringQuery);
        assert!(third.diagnostics.is_empty(), "{:?}", third.diagnostics);
        let trace = db.query_trace();
        assert_eq!(query_executions(&trace, "backend_item_plan"), 3);
        assert_eq!(query_executions(&trace, "backend_module_item_plan"), 0);
        assert_eq!(query_executions(&trace, "backend_module_finalization"), 6);
        assert_eq!(
            query_executions(&trace, "backend_finalization_task_context"),
            1
        );
        assert_eq!(query_executions(&trace, "backend_lowering"), 3);
        assert!(trace_has_dependency(
            &trace,
            "backend_lowering",
            "backend_module_finalization"
        ));
        assert!(trace_has_dependency(
            &trace,
            "backend_module_finalization",
            "backend_module_item_plan"
        ));
        assert!(trace_has_dependency(
            &trace,
            "backend_module_finalization",
            "backend_finalization_task_context"
        ));
    }

    #[test]
    fn backend_materializes_frontend_planned_source_functions() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module child;

fn helper() i32 {
    7
}

fn unused() i32 {
    9
}

fn main() i32 {
    helper() + child::value()
}
"#,
        );
        let module_id = fixture.entry_id();
        let child_id = fixture.add_child(
            module_id,
            "child",
            "child.nia",
            r#"
pub struct Value {
    number: i32,
}

pub fn value() i32 {
    let value = Value { number: 5 };
    value.number
}
"#,
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let facts = db.expect_get(ExecutableCheckedModuleFactsQuery);
        let module = facts
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module facts");
        let function = |name| {
            module
                .defs
                .defs
                .iter()
                .find_map(|(def_id, def)| {
                    (def.kind == nia_defs::DefKind::Function && def.name == sym(name))
                        .then_some(GlobalDefId { module_id, def_id })
                })
                .unwrap_or_else(|| panic!("missing function `{name}`"))
        };
        let helper = function("helper");
        let unused = function("unused");
        let main = function("main");

        let plan = db.expect_get(BackendModuleSourceItemPlanQuery(module_id));
        for items in [&plan.functions, &plan.globals, &plan.structs, &plan.unions] {
            assert!(items.windows(2).all(|pair| pair[0] < pair[1]), "{plan:?}");
            assert!(
                items.iter().all(|def_id| def_id.module_id == module_id),
                "{plan:?}"
            );
        }
        assert!(plan.functions.contains(&helper), "{plan:?}");
        assert!(plan.functions.contains(&main), "{plan:?}");
        assert!(!plan.functions.contains(&unused), "{plan:?}");
        assert!(plan.structs.is_empty(), "{plan:?}");

        let child_plan = db.expect_get(BackendModuleSourceItemPlanQuery(child_id));
        for items in [
            &child_plan.functions,
            &child_plan.globals,
            &child_plan.structs,
            &child_plan.unions,
        ] {
            assert!(
                items.windows(2).all(|pair| pair[0] < pair[1]),
                "{child_plan:?}"
            );
            assert!(
                items.iter().all(|def_id| def_id.module_id == child_id),
                "{child_plan:?}"
            );
        }
        assert_eq!(child_plan.functions.len(), 1, "{child_plan:?}");
        assert_eq!(child_plan.structs.len(), 1, "{child_plan:?}");

        let backend = db.expect_get(BackendLoweringQuery);
        assert!(backend.diagnostics.is_empty(), "{:?}", backend.diagnostics);
        let backend_module = backend
            .semantic
            .program
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry backend module");
        let functions = backend_module
            .functions
            .iter()
            .map(|function| function.def_id)
            .collect::<HashSet<_>>();
        assert!(functions.contains(&helper), "{functions:?}");
        assert!(functions.contains(&main), "{functions:?}");
        assert!(!functions.contains(&unused), "{functions:?}");

        let trace = db.query_trace();
        assert!(trace_has_dependency(
            &trace,
            "backend_lowering_inputs",
            "backend_module_source_item_plan"
        ));
        assert!(trace_has_dependency(
            &trace,
            "backend_module_source_item_plan",
            "executable_checked_module_facts"
        ));
    }

    #[test]
    fn codegen_reuses_per_function_lowering_between_mono_and_backend() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "fn helper() i32 { 1 } fn main() i32 { helper() }",
        );
        let db = query_db(fixture.program());

        let codegen = db.expect_get(CodegenProgramQuery);
        let trace = db.query_trace();

        assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
        let body_count = codegen
            .modules
            .iter()
            .map(|module| module.body_ir.function_bodies.len())
            .sum::<usize>();
        assert_eq!(
            query_executions(&trace, "lowered_function_body"),
            body_count
        );
        assert!(
            query_cache_hits(&trace, "lowered_function_body") >= body_count,
            "backend lowering should reuse monomorphization's function products"
        );
        assert!(trace_has_dependency(
            &trace,
            "monomorphization",
            "lowered_function_body"
        ));
        assert!(trace_has_dependency(
            &trace,
            "backend_lowering_inputs",
            "lowered_function_body"
        ));
        assert!(!trace_has_dependency(
            &trace,
            "codegen_program",
            "lowered_function_body"
        ));
        assert!(trace_has_dependency(
            &trace,
            "lowered_function_body",
            "executable_function_body"
        ));
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

    #[test]
    fn body_edit_keeps_unrelated_lowered_function_product_green() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "fn helper() i32 { 1 } fn main() i32 { helper() }",
        );
        let module_id = fixture.entry_id();
        let database = fixture.database();

        let first_codegen = database.codegen_program();
        assert!(
            first_codegen.diagnostics.is_empty(),
            "{:?}",
            first_codegen.diagnostics
        );
        let checked = database.db.expect_get(ExecutableCheckedModulesQuery);
        let module = checked
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module should be checked");
        let function = |name| {
            module
                .defs
                .defs
                .iter()
                .find_map(|(def_id, def)| {
                    (def.name == sym(name)).then_some(GlobalDefId { module_id, def_id })
                })
                .unwrap_or_else(|| panic!("missing function `{name}`"))
        };
        let helper = function("helper");
        let main = function("main");
        let fact_modules = database.db.expect_get(ExecutableCheckedModuleFactsQuery);
        let fact_module = fact_modules
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module facts should exist");
        assert!(
            fact_module.body_ir.function_bodies.is_empty(),
            "the executable facts aggregate must not produce checked bodies"
        );
        assert!(fact_modules.runtime_functions.contains(&helper));
        assert!(fact_modules.runtime_functions.contains(&main));
        let checked_helper = module
            .body_ir
            .function_bodies
            .get(&helper)
            .expect("helper should have a checked body");
        let checked_helper_product = database.db.expect_get(ExecutableFunctionBodyQuery(helper));
        let checked_helper_product = checked_helper_product
            .as_ref()
            .as_ref()
            .expect("helper checked-body product");
        assert!(Arc::ptr_eq(checked_helper, checked_helper_product));
        let first_helper = database.db.expect_get(LoweredFunctionBodyQuery(helper));
        let first_main = database.db.expect_get(LoweredFunctionBodyQuery(main));

        fixture.update_module_source(
            module_id,
            "fn helper() i32 { 2 } fn main() i32 { helper() }",
            SourceRevision(1),
        );
        database.update(CompileRequest::new(fixture.program()));
        let second_codegen = database.codegen_program();
        assert!(
            second_codegen.diagnostics.is_empty(),
            "{:?}",
            second_codegen.diagnostics
        );
        let second_helper = database.db.expect_get(LoweredFunctionBodyQuery(helper));
        let second_main = database.db.expect_get(LoweredFunctionBodyQuery(main));
        let trace = database.query_trace();

        assert!(!Arc::ptr_eq(&first_helper, &second_helper));
        assert!(Arc::ptr_eq(&first_main, &second_main));
        assert_eq!(query_executions(&trace, "executable_function_body"), 4);
        assert_eq!(query_executions(&trace, "lowered_function_body"), 3);
        assert!(query_green_validations(&trace, "lowered_function_body") >= 1);
    }

    #[test]
    fn global_edit_preserves_unrelated_static_init_semantic_value() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
static first: [4]u8 = [1, 2, 3, 4];
static second: [4]u8 = [5, 6, 7, 8];

fn main() u8 {
    first[0] + second[0]
}
"#,
        );
        let module_id = fixture.entry_id();
        let database = fixture.database();

        let first_codegen = database.codegen_program();
        assert!(
            first_codegen.diagnostics.is_empty(),
            "{:?}",
            first_codegen.diagnostics
        );
        let checked = database.db.expect_get(ExecutableCheckedModulesQuery);
        let module = checked
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module should be checked");
        let global = |name| {
            module
                .defs
                .defs
                .iter()
                .find_map(|(def_id, def)| {
                    (def.kind == nia_defs::DefKind::Global && def.name == sym(name))
                        .then_some(GlobalDefId { module_id, def_id })
                })
                .unwrap_or_else(|| panic!("missing global `{name}`"))
        };
        let first = global("first");
        let second = global("second");
        let fact_modules = database.db.expect_get(ExecutableCheckedModuleFactsQuery);
        let fact_module = fact_modules
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module facts should exist");
        assert!(
            fact_module.body_ir.global_inits.is_empty(),
            "the executable facts aggregate must not produce static initializer payloads"
        );
        assert!(fact_modules.runtime_globals.contains(&first));
        assert!(fact_modules.runtime_globals.contains(&second));
        let aggregate_first = module
            .body_ir
            .global_inits
            .get(&first)
            .expect("first should have a static initializer");
        let first_item = database.db.expect_get(ExecutableStaticInitQuery(first));
        let first_payload = first_item
            .as_ref()
            .as_ref()
            .expect("first static initializer product");
        assert!(Arc::ptr_eq(aggregate_first, first_payload));
        let first_second = database.db.expect_get(ExecutableStaticInitQuery(second));

        fixture.update_module_source(
            module_id,
            r#"
static first: [4]u8 = [9, 2, 3, 4];
static second: [4]u8 = [5, 6, 7, 8];

fn main() u8 {
    first[0] + second[0]
}
"#,
            SourceRevision(1),
        );
        database.update(CompileRequest::new(fixture.program()));
        let second_codegen = database.codegen_program();
        assert!(
            second_codegen.diagnostics.is_empty(),
            "{:?}",
            second_codegen.diagnostics
        );
        let second_first = database.db.expect_get(ExecutableStaticInitQuery(first));
        let second_second = database.db.expect_get(ExecutableStaticInitQuery(second));
        let checked = database.db.expect_get(ExecutableCheckedModulesQuery);
        let module = checked
            .iter()
            .find(|module| module.id == module_id)
            .expect("updated entry module should be checked");
        let aggregate_second = module
            .body_ir
            .global_inits
            .get(&second)
            .expect("second should retain a static initializer");
        let second_payload = second_second
            .as_ref()
            .as_ref()
            .expect("updated second static initializer product");
        let trace = database.query_trace();

        assert!(!Arc::ptr_eq(&first_item, &second_first));
        assert_eq!(first_second.as_ref(), second_second.as_ref());
        assert!(Arc::ptr_eq(aggregate_second, second_payload));
        assert_eq!(query_executions(&trace, "executable_static_init"), 4);
    }

    #[test]
    fn static_init_ref_summary_drives_reachability_without_aggregate_payload() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
struct Helper {}

extend Helper {
    fn value() i32 {
        7
    }
}

static callback: &fn() i32 = &Helper::value;

fn main() i32 {
    callback()
}
"#,
        );
        let db = query_db(fixture.program());

        let codegen = db.expect_get(CodegenProgramQuery);
        assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
        let facts = db.expect_get(ExecutableCheckedModuleFactsQuery);
        let module = facts.modules.first().expect("entry module facts");
        assert!(module.body_ir.global_inits.is_empty());
        let def_id = |name| {
            module
                .defs
                .defs
                .iter()
                .find_map(|(def_id, def)| {
                    (def.name == sym(name)).then_some(GlobalDefId {
                        module_id: module.id,
                        def_id,
                    })
                })
                .unwrap_or_else(|| panic!("missing definition `{name}`"))
        };
        let helper = def_id("value");
        let callback = def_id("callback");

        assert!(facts.runtime_functions.contains(&helper));
        assert!(facts.runtime_globals.contains(&callback));
        let init = db.expect_get(ExecutableStaticInitQuery(callback));
        assert!(
            matches!(
                init.as_ref().as_deref(),
                Some(nia_static_ir::StaticInit::AddrOfFunction { function, .. })
                    if *function == helper
            ),
            "{init:?}"
        );
        assert!(
            codegen
                .backend_lowering
                .program
                .modules
                .iter()
                .flat_map(|module| &module.functions)
                .any(|function| function.def_id == helper),
            "the static reference summary must keep helper reachable"
        );
    }

    #[test]
    fn local_static_item_uses_owner_function_facts_for_associated_function_reference() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
struct Helper {}

extend Helper {
    fn value() i32 {
        7
    }
}

fn invoke() i32 {
    static callback: &fn() i32 = &Helper::value;
    callback()
}

fn main() i32 {
    invoke()
}
"#,
        );
        let db = query_db(fixture.program());

        let codegen = db.expect_get(CodegenProgramQuery);
        assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
        let facts = db.expect_get(ExecutableCheckedModuleFactsQuery);
        let module = facts.modules.first().expect("entry module facts");
        assert!(module.body_ir.global_inits.is_empty());
        let def_id = |name, kind| {
            module
                .defs
                .defs
                .iter()
                .find_map(|(def_id, def)| {
                    (def.name == sym(name) && def.kind == kind).then_some(GlobalDefId {
                        module_id: module.id,
                        def_id,
                    })
                })
                .unwrap_or_else(|| panic!("missing {kind:?} definition `{name}`"))
        };
        let helper = def_id("value", nia_defs::DefKind::Method);
        let callback = def_id("callback", nia_defs::DefKind::Global);

        assert!(facts.runtime_functions.contains(&helper));
        assert!(facts.runtime_globals.contains(&callback));
        let init = db.expect_get(ExecutableStaticInitQuery(callback));
        assert!(
            matches!(
                init.as_ref().as_deref(),
                Some(nia_static_ir::StaticInit::AddrOfFunction { function, .. })
                    if *function == helper
            ),
            "{init:?}"
        );
        assert!(
            codegen
                .backend_lowering
                .program
                .modules
                .iter()
                .flat_map(|module| &module.functions)
                .any(|function| function.def_id == helper),
            "the local static reference summary must keep helper reachable"
        );
    }

    #[test]
    fn codegen_public_adapter_reuses_large_product_handles() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 1 }");
        let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));

        let cached = database.db.expect_get(CodegenProgramQuery);
        let owned = database.codegen_program();

        assert!(Arc::ptr_eq(
            &cached.monomorphization,
            &owned.monomorphization
        ));
        assert!(Arc::ptr_eq(
            &cached.backend_lowering,
            &owned.backend_lowering
        ));
    }

    #[test]
    fn codegen_preparation_does_not_cross_backend_aggregate_barrier() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 1 }");
        let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));

        let preparation = database.codegen_preparation();

        assert!(preparation.diagnostics.is_empty());
        let trace = database.query_trace();
        assert_eq!(query_executions(&trace, "codegen_preparation"), 1);
        assert_eq!(query_executions(&trace, "backend_lowering"), 0);
        assert_eq!(query_executions(&trace, "backend_module_finalization"), 0);
    }

    #[test]
    fn scoped_backend_schedule_exposes_each_module_before_aggregate_finish() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "module helper; using entry::helper; fn main() i32 { helper::value() }",
        );
        let entry = fixture.entry_id();
        let helper = fixture.add_child(entry, "helper", "helper.nia", "pub fn value() i32 { 1 }");
        let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));
        let preparation = database.codegen_preparation();
        assert!(preparation.diagnostics.is_empty());

        let lowering = database
            .with_backend_finalization_schedule(|schedule| {
                let mut schedule = schedule.expect("healthy preparation must produce a schedule");
                let store = schedule.module_store();
                assert!(store.get(entry).is_none());
                assert!(store.get(helper).is_none());

                let ready = schedule
                    .wait_next()
                    .expect("backend finalization query")
                    .expect("first backend module");
                assert!(store.get(ready.module_id()).is_some());
                let other = if ready.module_id() == entry {
                    helper
                } else {
                    entry
                };
                assert!(store.get(other).is_none());
                schedule.finish()
            })
            .expect("backend finalization schedule")
            .expect("backend finalization queries");

        assert_eq!(lowering.program.modules.len(), 2);
        assert!(
            lowering
                .program
                .modules
                .iter()
                .any(|module| module.id == entry)
        );
        assert!(
            lowering
                .program
                .modules
                .iter()
                .any(|module| module.id == helper)
        );
        let trace = database.query_trace();
        assert_eq!(query_executions(&trace, "backend_lowering"), 0);
        assert_eq!(query_executions(&trace, "backend_module_finalization"), 2);
    }

    #[test]
    fn backend_definition_manifest_precedes_finalization_at_every_optimization_level() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module geom;
using entry::geom;

fn main() i32 {
    let mut point: geom::Point = { x: 40, y: 2 };
    point.x + point.y
}
"#,
        );
        let entry = fixture.entry_id();
        let geom = fixture.add_child(
            entry,
            "geom",
            "geom.nia",
            r#"
pub struct Point {
    x: i32,
    y: i32,
}
"#,
        );

        for level in [
            NiaOptimizationLevel::O0,
            NiaOptimizationLevel::O1,
            NiaOptimizationLevel::O2,
            NiaOptimizationLevel::O3,
            NiaOptimizationLevel::Os,
            NiaOptimizationLevel::Oz,
        ] {
            let mut program = fixture.program();
            program.runtime = RuntimeModel::FreestandingExecutable;
            let database =
                CompilerDatabase::new(CompileRequest::new(program).with_optimization(level));
            let preparation = database.codegen_preparation();
            assert!(
                preparation.diagnostics.is_empty(),
                "{level:?}: {:?}",
                preparation.diagnostics
            );
            let defs = database.db.expect_get(FullModuleDefsQuery(geom));
            let point =
                defs.semantic
                    .defs
                    .iter()
                    .find_map(|(def_id, def)| {
                        (def.name == sym("Point") && def.kind == nia_defs::DefKind::Struct)
                            .then_some(GlobalDefId {
                                module_id: geom,
                                def_id,
                            })
                    })
                    .expect("Point definition");

            database
                .with_backend_finalization_schedule(|schedule| {
                    let mut schedule =
                        schedule.expect("healthy preparation must produce a schedule");
                    assert_eq!(schedule.owner_directory().item_owner(point), Some(geom));
                    assert!(schedule.module_store().get(geom).is_none());
                    while schedule
                        .wait_next()
                        .expect("backend finalization query")
                        .is_some()
                    {}
                    let lowering = schedule.finish().expect("backend finalization queries");
                    let geom_module = lowering
                        .program
                        .modules
                        .iter()
                        .find(|module| module.id == geom)
                        .expect("finalized geom module");
                    assert!(geom_module.structs.iter().any(|item| item.def_id == point));
                })
                .expect("backend finalization schedule");
        }
    }

    #[test]
    fn executable_checked_modules_reuse_filtered_const_inputs() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
const fn len() usize {
    4
}

fn unused() i32 {
    missing_symbol
}

fn main() i32 {
    let mut values: [len()]i32 = [0; len()];
    values.len() as i32
}
"#,
        );
        let module_id = fixture.entry_id();
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let facts = db.expect_get(ExecutableCheckedModuleFactsQuery);
        assert!(facts.const_modules.contains_key(&module_id));
        assert!(
            facts
                .runtime_functions
                .iter()
                .chain(&facts.runtime_globals)
                .all(|def_id| facts.const_modules.contains_key(&def_id.module_id))
        );
        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module should be executable-reachable");
        let trace = db.query_trace();

        assert!(
            module.body_diagnostics.is_empty(),
            "reachable const functions must remain available to executable body checking: {:?}",
            module.body_diagnostics
        );
        assert!(
            module
                .const_eval
                .array_lengths
                .values()
                .any(|length| *length == 4),
            "filtered executable const phases should retain reachable array lengths"
        );
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_body_check"
                && matches!(
                    dependency.to.name,
                    "const_values" | "const_array_lengths" | "const_typed_facts"
                )
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_body_check"
                && dependency.to.name == "program_trait_solving_signatures"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_checked_modules"
                && matches!(dependency.to.name, "const" | "const_enum_values")
        }));
    }

    #[test]
    fn executable_full_lowering_reuses_explicit_and_inferred_const_types() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
const explicit: usize = 19usize;
const inferred = 4usize;

fn main() usize {
    explicit + inferred
}
"#,
        );
        let module_id = fixture.entry_id();
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module should be executable-reachable");

        assert!(
            module.body_diagnostics.is_empty(),
            "prechecked const types must remain available during full body lowering: {:?}",
            module.body_diagnostics
        );
        assert_eq!(module.semantic_facts.const_types.len(), 2);
        assert_eq!(module.body_ir.function_bodies.len(), 1);
    }

    #[test]
    fn executable_body_check_follows_same_module_call_closure() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
fn f3() i32 {
    3
}

fn f2() i32 {
    f3()
}

fn f1() i32 {
    f2()
}

fn main() i32 {
    f1()
}
"#,
        );
        let module_id = fixture.entry_id();
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module should be executable-reachable");
        assert_eq!(
            module.body_ir.function_bodies.len(),
            4,
            "same-module executable body check should retain the whole call closure"
        );
        assert!(
            module.body_diagnostics.is_empty(),
            "same-module executable call closure should check without diagnostics: {:?}",
            module.body_diagnostics
        );
    }

    #[test]
    fn executable_filtered_const_resolves_forwarded_array_len_values() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module facade;
using entry::facade;

fn main() i32 {
    let mut values: [facade::LEN]u8 = [0; facade::LEN];
    values.len() as i32
}
"#,
        );
        let entry_id = fixture.entry_id();
        let facade_id = fixture.add_child(
            entry_id,
            "facade",
            "facade.nia",
            r#"
module raw;
using self::raw;

pub const LEN: usize = raw::LEN;
"#,
        );
        fixture.add_child(
            facade_id,
            "raw",
            "facade/raw.nia",
            r#"
pub const LEN: usize = 4usize;
"#,
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let entry = modules
            .iter()
            .find(|module| module.id == entry_id)
            .expect("entry module should be executable-reachable");

        assert!(
            entry.body_diagnostics.is_empty(),
            "filtered executable body checking should resolve forwarded const array lengths: {:?}",
            entry.body_diagnostics
        );
        assert!(
            entry
                .const_eval
                .array_lengths
                .values()
                .any(|length| *length == 4),
            "filtered executable const should evaluate forwarded array length"
        );
    }

    #[test]
    fn executable_filtered_const_resolves_local_forwarded_array_len_in_method_body() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module raw;
using entry::raw;

const LEN: usize = raw::LEN;

struct Box {}

extend Box {
    fn value(&self) usize {
        let mut values: [LEN]u8 = [_]u8[0; LEN];
        values.len()
    }
}

fn main() usize {
    let box = Box {};
    box.value()
}
"#,
        );
        let entry_id = fixture.entry_id();
        fixture.add_child(
            entry_id,
            "raw",
            "raw.nia",
            r#"
pub const LEN: usize = 4usize;
"#,
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let entry = modules
            .iter()
            .find(|module| module.id == entry_id)
            .expect("entry module should be executable-reachable");

        assert!(
            entry.body_diagnostics.is_empty(),
            "filtered executable body checking should resolve local forwarded array lengths used in method bodies: {:?}",
            entry.body_diagnostics
        );
        assert!(
            entry
                .const_eval
                .array_lengths
                .values()
                .any(|length| *length == 4),
            "filtered executable const should evaluate local forwarded method-body array length"
        );
    }

    #[test]
    fn executable_incremental_body_check_preserves_extension_method_receiver_types() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module writer;
using entry::writer;

fn main() i32 {
    let mut sink = writer::Sink::init();
    if !value = sink.write(b"ok") {
        value as i32
    } or error! {
        0
    }
}
"#,
        );
        let entry_id = fixture.entry_id();
        let writer_id = fixture.add_child(
            entry_id,
            "writer",
            "writer.nia",
            r#"
pub trait Writer {
    type Error;

    fn short_write(&self) Error;

    fn write(&mut self, bytes: &[u8]) Error!usize;
}

pub enum WriteError: i32 {
    Short = 1,
    _,
}

pub struct Sink {}

extend Sink {
    pub fn init() Sink {
        {}
    }
}

extend Sink : Writer {
    type Error = WriteError;

    pub fn short_write(&self) Error {
        WriteError::Short
    }

    pub fn write(&mut self, bytes: &[u8]) Error!usize {
        if bytes.len() == 0 {
            return self.short_write()!;
        }
        !bytes.len()
    }
}
"#,
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let writer = modules
            .iter()
            .find(|module| module.id == writer_id)
            .expect("writer module should be executable-reachable");
        let write_def = writer
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == sym("write") && def.kind == nia_defs::DefKind::Method)
                    .then_some(def_id)
            })
            .expect("write method should be defined");
        let write_id = GlobalDefId {
            module_id: writer_id,
            def_id: write_def,
        };
        let write_body = writer
            .body_ir
            .function_bodies
            .get(&write_id)
            .expect("write method should have a checked body");
        let self_ty = write_body
            .locals
            .iter()
            .find(|local| {
                local.name.is_self_value() && local.kind == nia_body_ir::TypedLocalKind::Param
            })
            .map(|local| local.ty)
            .expect("write method should have a self param");
        assert!(
            !matches!(db.context().type_store.get(self_ty), Some(TyKind::Error)),
            "reachable extension method receiver/params should not collapse to error types"
        );
    }

    #[test]
    fn trait_signature_subset_resolves_local_extend_target_types() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
trait Writer {
    type Error;
    fn write(&mut self) Error!void;
}

enum WriteError: i32 {
    Bad = 1,
    _,
}

struct Sink {}

extend Sink : Writer {
    type Error = WriteError;

    fn write(&mut self) Error!void {
        !{}
    }
}
"#,
        );
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let signatures = db.expect_get(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let impl_signature = signatures
            .semantic
            .trait_impls
            .iter()
            .find(|impl_signature| !impl_signature.methods.is_empty())
            .expect("trait impl should be collected");

        assert!(
            !matches!(
                db.context().type_store.get(impl_signature.target_ty),
                Some(TyKind::Error)
            ),
            "trait signature subset should resolve local extend target types"
        );
    }

    #[test]
    fn trait_signature_subset_resolves_imported_extend_target_types() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module platform;
using entry::platform;

trait IntoError[Target] {
    fn into_error(self) Target;
}

enum Error: i32 {
    Bad = 1,
    _,
}

extend platform::Errno : IntoError[Error] {
    fn into_error(self) Error {
        Error::Bad
    }
}
"#,
        );
        let entry_id = fixture.entry_id();
        fixture.add_child(
            entry_id,
            "platform",
            "platform.nia",
            r#"
pub enum Errno: i32 {
    Bad = 1,
    _,
}
"#,
        );
        let db = query_db(fixture.program());

        let signatures = db.expect_get(SignatureItemSignaturesQuery(
            entry_id,
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let impl_signature = signatures
            .semantic
            .trait_impls
            .iter()
            .find(|impl_signature| !impl_signature.methods.is_empty())
            .expect("trait impl should be collected");

        assert!(
            !matches!(
                db.context().type_store.get(impl_signature.target_ty),
                Some(TyKind::Error)
            ),
            "trait signature subset should resolve imported extend target types"
        );
    }

    #[test]
    fn trait_signature_subset_resolves_reexported_extend_target_types() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module platform;
using entry::platform;

trait IntoError[Target] {
    fn into_error(self) Target;
}

enum Error: i32 {
    Bad = 1,
    _,
}

extend platform::Errno : IntoError[Error] {
    fn into_error(self) Error {
        Error::Bad
    }
}
"#,
        );
        let entry_id = fixture.entry_id();
        let platform_id = fixture.add_child(
            entry_id,
            "platform",
            "platform.nia",
            r#"
module types;
using entry::platform::types;

pub using types::{Errno};
"#,
        );
        fixture.add_child(
            platform_id,
            "types",
            "types.nia",
            r#"
pub enum Errno: i32 {
    Bad = 1,
    _,
}
"#,
        );
        let db = query_db(fixture.program());

        let signatures = db.expect_get(SignatureItemSignaturesQuery(
            entry_id,
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let impl_signature = signatures
            .semantic
            .trait_impls
            .iter()
            .find(|impl_signature| !impl_signature.methods.is_empty())
            .expect("trait impl should be collected");

        assert!(
            !matches!(
                db.context().type_store.get(impl_signature.target_ty),
                Some(TyKind::Error)
            ),
            "trait signature subset should resolve re-exported extend target types"
        );
    }

    #[test]
    fn executable_incremental_body_check_preserves_reexported_trait_witness_receiver_types() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module platform;
using entry::platform;

trait IntoError[Target] {
    fn into_error(self) Target;
}

extend[T, Source, Target] Source!T
where Source: IntoError[Target]
{
    fn cast_error(self) Target!T {
        if !ok = self {
            !ok
        } or error! {
            error.into_error()!
        }
    }
}

enum Error: i32 {
    Bad = 1,
    _,
}

extend platform::Errno : IntoError[Error] {
    fn into_error(self) Error {
        Error::Bad
    }
}

fn fail() platform::Errno!i32 {
    platform::Errno::Bad!
}

fn main() Error!i32 {
    fail().cast_error()
}
"#,
        );
        let entry_id = fixture.entry_id();
        let platform_id = fixture.add_child(
            entry_id,
            "platform",
            "platform.nia",
            r#"
module types;
using entry::platform::types;

pub using types::{Errno};
"#,
        );
        fixture.add_child(
            platform_id,
            "types",
            "types.nia",
            r#"
pub enum Errno: i32 {
    Bad = 1,
    _,
}
"#,
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == entry_id)
            .expect("entry module should be executable-reachable");
        assert!(
            module.body_diagnostics.is_empty(),
            "generic extension wrapper diagnostics should stay clean: {:?}",
            module.body_diagnostics
        );
        let into_error = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == sym("into_error") && def.kind == nia_defs::DefKind::Method).then_some(
                    GlobalDefId {
                        module_id: entry_id,
                        def_id,
                    },
                )
            })
            .expect("into_error method should be defined");
        let body = module
            .body_ir
            .function_bodies
            .get(&into_error)
            .expect("into_error should have a checked body");
        let self_ty = body
            .locals
            .iter()
            .find(|local| {
                local.name.is_self_value() && local.kind == nia_body_ir::TypedLocalKind::Param
            })
            .map(|local| local.ty)
            .expect("into_error should have a self param");
        assert!(
            !matches!(db.context().type_store.get(self_ty), Some(TyKind::Error)),
            "re-exported trait witness receiver should not collapse to error"
        );
    }

    #[test]
    fn executable_reachability_expands_where_predicates_through_generic_extension_wrappers() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module error;
module facade;
using entry::error;
using entry::facade;

enum Error: i32 {
    Bad = 1,
    _,
}

struct Source {
    value: i32,
}

struct Target {
    value: i32,
}

extend Source : error::IntoError[Target] {
    fn into_error(self) Target {
        Target { value: self.value }
    }
}

fn main() i32 {
    let value: Source!i32 = Source { value: 1 }!;
    if !ok = value.cast_error() {
        ok
    } or error! {
        error.value
    }
}
"#,
        );
        let entry_id = fixture.entry_id();
        fixture.add_child(
            entry_id,
            "error",
            "error.nia",
            r#"
pub trait IntoError[Target] {
    fn into_error(self) Target;
}

extend[T, Source, Target] Source!T
where Source: IntoError[Target]
{
    pub fn cast_error(self) Target!T {
        if !ok = self {
            !ok
        } or error! {
            error.into_error()!
        }
    }
}
"#,
        );
        fixture.add_child(
            entry_id,
            "facade",
            "facade.nia",
            r#"
using entry::error;
"#,
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == entry_id)
            .expect("entry module should be executable-reachable");
        let into_error = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == sym("into_error") && def.kind == nia_defs::DefKind::Method).then_some(
                    GlobalDefId {
                        module_id: entry_id,
                        def_id,
                    },
                )
            })
            .expect("into_error method should be defined");

        assert!(
            module.body_ir.function_bodies.contains_key(&into_error),
            "generic extension wrappers should make where-predicate trait witnesses executable-reachable"
        );
    }

    #[test]
    fn executable_reachability_expands_generic_trait_calls_to_cross_module_impl_bodies() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module error;
module impls;
using entry::error;
using entry::impls;

fn main() i32 {
    let value: impls::Source!i32 = impls::Source { value: 1 }!;
    if !ok = value.cast_error() {
        ok
    } or error! {
        error.value
    }
}
"#,
        );
        let entry_id = fixture.entry_id();
        fixture.add_child(
            entry_id,
            "error",
            "error.nia",
            r#"
pub trait IntoError[Target] {
    fn into_error(self) Target;
}

extend[T, Source, Target] Source!T
where Source: IntoError[Target]
{
    pub fn cast_error(self) Target!T {
        if !ok = self {
            !ok
        } or error! {
            error.into_error()!
        }
    }
}
"#,
        );
        let impls_id = fixture.add_child(
            entry_id,
            "impls",
            "impls.nia",
            r#"
using entry::error;

pub struct Source {
    value: i32,
}

pub struct Target {
    value: i32,
}

extend Source : error::IntoError[Target] {
    fn into_error(self) Target {
        Target { value: self.value }
    }
}
"#,
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == impls_id)
            .expect("impl module should be executable-reachable");
        let into_error = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == sym("into_error") && def.kind == nia_defs::DefKind::Method).then_some(
                    GlobalDefId {
                        module_id: impls_id,
                        def_id,
                    },
                )
            })
            .expect("cross-module into_error method should be defined");

        assert!(
            module.body_ir.function_bodies.contains_key(&into_error),
            "generic trait calls should make cross-module impl method bodies executable-reachable"
        );
    }

    #[test]
    fn executable_reachability_expands_generic_trait_calls_from_incremental_wrapper_bodies() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module error;
module impls;
using entry::error;
using entry::impls;

fn main() i32 {
    let value: impls::Source!i32 = impls::Source { value: 1 }!;
    if !ok = value.as_target_error() {
        ok
    } or error! {
        error.value
    }
}
"#,
        );
        let entry_id = fixture.entry_id();
        fixture.add_child(
            entry_id,
            "error",
            "error.nia",
            r#"
pub trait IntoError[Target] {
    fn into_error(self) Target;
}

extend[T, Source, Target] Source!T
where Source: IntoError[Target]
{
    pub fn cast_error(self) Target!T {
        if !ok = self {
            !ok
        } or error! {
            error.into_error()!
        }
    }
}
"#,
        );
        let impls_id = fixture.add_child(
            entry_id,
            "impls",
            "impls.nia",
            r#"
using entry::error;

pub struct Source {
    value: i32,
}

pub struct Target {
    value: i32,
}

extend Source : error::IntoError[Target] {
    fn into_error(self) Target {
        Target { value: self.value }
    }
}

extend[T] Source!T {
    pub fn as_target_error(self) Target!T {
        self.cast_error()
    }
}
"#,
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == impls_id)
            .expect("impl module should be executable-reachable");
        let into_error = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == sym("into_error") && def.kind == nia_defs::DefKind::Method).then_some(
                    GlobalDefId {
                        module_id: impls_id,
                        def_id,
                    },
                )
            })
            .expect("cross-module into_error method should be defined");

        assert!(
            module.body_ir.function_bodies.contains_key(&into_error),
            "generic wrapper bodies checked after incremental reachability must still expand their trait witnesses"
        );
    }

    #[test]
    fn executable_checked_modules_include_reachable_builtin_trait_witness_bodies() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
struct Counter {
    current: i32,
    end: i32,
}

extend Counter : Iterator {
    type Item = i32;

    fn next(&mut self) ?i32 {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}

fn main() i32 {
    let mut total = 0;
    let mut iter = Counter { current: 0, end: 3 };
    for value in iter {
        total += value;
    }
    total
}
"#,
        );
        let module_id = fixture.entry_id();
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module should be executable-reachable");
        let next = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Method && def.name == sym("next"))
                    .then_some(GlobalDefId { module_id, def_id })
            })
            .expect("Iterator witness method");

        assert!(
            module.body_ir.function_bodies.contains_key(&next),
            "executable body checking must include builtin trait witness bodies"
        );
    }

    #[test]
    fn executable_checked_modules_do_not_body_check_unmatched_builtin_trait_witnesses() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
struct Counter {
    current: i32,
    end: i32,
}

extend Counter : Iterator {
    type Item = i32;

    fn next(&mut self) ?i32 {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}

struct Unused {}

extend Unused : Iterator {
    type Item = i32;

    fn next(&mut self) ?i32 {
        ?missing_symbol
    }
}

fn main() i32 {
    let mut total = 0;
    let mut iter = Counter { current: 0, end: 3 };
    for value in iter {
        total += value;
    }
    total
}
"#,
        );
        let module_id = fixture.entry_id();
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module should be executable-reachable");
        let unused_next = module
            .defs
            .defs
            .iter()
            .filter_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Method && def.name == sym("next"))
                    .then_some(GlobalDefId { module_id, def_id })
            })
            .find(|def_id| !module.body_ir.function_bodies.contains_key(def_id))
            .expect("unmatched Iterator witness method");

        assert!(
            !module.body_ir.function_bodies.contains_key(&unused_next),
            "executable reachability should not include builtin trait witnesses for unmatched receiver types"
        );
        assert!(
            module.body_diagnostics.is_empty(),
            "unmatched builtin trait witness diagnostics should not block executable checking: {:?}",
            module.body_diagnostics
        );
    }

    #[test]
    fn executable_checked_modules_do_not_body_check_unused_trait_witness_methods() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
trait Ops {
    fn used(self) i32;
    fn unused(self) i32;
}

struct Value {}

extend Value : Ops {
    fn used(self) i32 {
        1
    }

    fn unused(self) i32 {
        missing_symbol
    }
}

fn main() i32 {
    let value = Value {};
    value.used()
}
"#,
        );
        let module_id = fixture.entry_id();
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module should be executable-reachable");
        let unused = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Method && def.name == sym("unused"))
                    .then_some(GlobalDefId { module_id, def_id })
            })
            .expect("unused witness method");

        assert!(
            !module.body_ir.function_bodies.contains_key(&unused),
            "executable body checking should not include unused trait witness bodies"
        );
    }

    #[test]
    fn executable_checked_modules_include_trait_witnesses_required_by_generic_where_predicates() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
trait IntoError[Target] {
    fn into_error(self) Target;
}

extend[T, Source, Target] Source!T
where Source: IntoError[Target]
{
    fn cast_error(self) Target!T {
        if !ok = self {
            !ok
        } or error! {
            error.into_error()!
        }
    }
}

struct Source {
    value: i32,
}

struct Target {
    value: i32,
}

extend Source : IntoError[Target] {
    fn into_error(self) Target {
        Target { value: self.value }
    }
}

struct Unused {}

extend Unused : IntoError[Target] {
    fn into_error(self) Target {
        missing_symbol
    }
}

fn main() i32 {
    let value: Source!i32 = Source { value: 1 }!;
    if !ok = value.cast_error() {
        ok
    } or error! {
        error.value
    }
}
"#,
        );
        let module_id = fixture.entry_id();
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module should be executable-reachable");
        let into_error_methods = module
            .defs
            .defs
            .iter()
            .filter_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Method && def.name == sym("into_error"))
                    .then_some(GlobalDefId { module_id, def_id })
            })
            .collect::<Vec<_>>();
        let reachable_into_error_count = into_error_methods
            .iter()
            .filter(|def_id| module.body_ir.function_bodies.contains_key(def_id))
            .count();

        assert_eq!(
            reachable_into_error_count, 1,
            "generic where-predicate closure should include only the matching IntoError witness"
        );
        assert!(
            module.body_diagnostics.is_empty(),
            "unmatched IntoError witness diagnostics should not block executable checking: {:?}",
            module.body_diagnostics
        );
    }

    #[test]
    fn executable_checked_modules_include_trait_witnesses_required_by_default_method_body() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
trait Writer {
    type Error;

    fn short_write(&self) Error;

    fn write(&mut self) Error!usize;

    fn write_all(&mut self) Error!void {
        let n = self.write().?;
        if n == 0usize {
            return self.short_write()!;
        }
        !{}
    }
}

struct FileWriter {
    value: i32,
}

extend FileWriter : Writer {
    type Error = i32;

    fn short_write(&self) Error {
        1
    }

    fn write(&mut self) Error!usize {
        self.value = 2;
        !1usize
    }
}

struct Unused {}

extend Unused : Writer {
    type Error = i32;

    fn short_write(&self) Error {
        missing_symbol
    }

    fn write(&mut self) Error!usize {
        missing_symbol
    }
}

fn main() i32!i32 {
    let mut writer = FileWriter { value: 0 };
    writer.write_all().?;
    !writer.value
}
"#,
        );
        let module_id = fixture.entry_id();
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module should be executable-reachable");
        let checked_witness_names = module
            .defs
            .defs
            .iter()
            .filter_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Method
                    && module
                        .body_ir
                        .function_bodies
                        .contains_key(&GlobalDefId { module_id, def_id }))
                .then_some(def.name)
            })
            .collect::<Vec<_>>();

        assert!(
            checked_witness_names.contains(&sym("write")),
            "default method reachability should include concrete write witness: {checked_witness_names:?}"
        );
        assert!(
            checked_witness_names.contains(&sym("short_write")),
            "default method reachability should include concrete short_write witness: {checked_witness_names:?}"
        );
        assert!(
            module.body_diagnostics.is_empty(),
            "unmatched Writer witness diagnostics should not block executable checking: {:?}",
            module.body_diagnostics
        );
    }

    #[test]
    fn executable_checked_modules_do_not_body_check_unreachable_globals() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
static unused = missing_symbol;

fn main() i32 {
    0
}
"#,
        );
        let module_id = fixture.entry_id();
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module should be executable-reachable");
        let unused = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Global && def.name == sym("unused"))
                    .then_some(GlobalDefId { module_id, def_id })
            })
            .expect("unused global");

        assert!(
            module.body_diagnostics.is_empty(),
            "unreachable global body diagnostics should not block executable checking: {:?}",
            module.body_diagnostics
        );
        assert!(
            !module.body_ir.global_inits.contains_key(&unused),
            "unreachable global initializers should not be retained for executable codegen"
        );
    }

    #[test]
    fn executable_backend_lowering_skips_unreachable_recursive_aggregates() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
struct Recursive {
    next: Recursive,
}

fn unused(value: Recursive) i32 {
    1
}

fn main() i32 {
    0
}
"#,
        );
        let module_id = fixture.entry_id();
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module should be executable-reachable");
        assert!(
            resolve_diagnostic_bundle(db.context(), &module.layout_diagnostics).is_empty(),
            "unreachable recursive aggregate should not force layout diagnostics: {:?}",
            resolve_diagnostic_bundle(db.context(), &module.layout_diagnostics)
        );

        let backend_lowering = db.expect_get(BackendLoweringQuery);
        let backend_module = backend_lowering
            .semantic
            .program
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module should be backend-lowered");
        let recursive = sym("Recursive");
        assert!(
            backend_module
                .structs
                .iter()
                .all(|item| item.name != recursive),
            "unreachable recursive aggregate should not be lowered for codegen"
        );
    }

    #[test]
    fn executable_backend_lowering_uses_canonical_external_extension_where_predicates() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module ext;
module bounds;
using entry::ext;
using entry::bounds;

fn main() i32 {
    let value = ext::Box[bounds::Token]::init(bounds::Token {});
    value.get()
}
"#,
        );
        let entry_id = fixture.entry_id();
        fixture.add_child(
            entry_id,
            "ext",
            "ext.nia",
            r#"
using entry::bounds;

pub struct Box[T]
where T: bounds::Marker
{
    value: T,
}

extend[T] Box[T]
where T: bounds::Marker
{
    pub fn init(value: T) Box[T] {
        { value: value }
    }

    pub fn get(self) i32 {
        1
    }
}
"#,
        );
        fixture.add_child(
            entry_id,
            "bounds",
            "bounds.nia",
            r#"
pub trait Marker {}

pub struct Token {}

extend Token : Marker {}
"#,
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let backend_lowering = db.expect_get(BackendLoweringQuery);

        assert!(
            backend_lowering.diagnostics.is_empty(),
            "backend lowering should import external extension owner predicates without diagnostics: {:?}",
            backend_lowering.diagnostics
        );
    }

    #[test]
    fn executable_backend_lowering_includes_cross_module_trait_default_vtable_instances() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module module1;
module module2;
using entry::module1;
using entry::module2;

fn main() i32 {
    let mut page = module2::Page::init();
    let allocator: &mut module1::Allocator = &mut page;
    allocator.remap()
}
"#,
        );
        let entry_id = fixture.entry_id();
        let module1_id = fixture.add_child(
            entry_id,
            "module1",
            "module1.nia",
            r#"
pub trait Allocator {
    fn alloc(&mut self) i32;

    fn remap(&mut self) i32 {
        _ = self;
        helper()
    }
}

fn helper() i32 {
    7
}
"#,
        );
        fixture.add_child(
            entry_id,
            "module2",
            "module2.nia",
            r#"
using entry::module1;
using module1::Allocator;

pub struct Page {}

extend Page {
    pub fn init() Page {
        {}
    }
}

extend Page : Allocator {
    fn alloc(&mut self) i32 {
        _ = self;
        7
    }
}
"#,
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let codegen = CompilerDatabase::new(
            CompileRequest::new(loaded).with_optimization(NiaOptimizationLevel::O1),
        )
        .codegen_program();

        let backend_lowering = &codegen.backend_lowering;

        assert!(
            backend_lowering.diagnostics.is_empty(),
            "backend lowering should not report diagnostics: {:?}",
            backend_lowering.diagnostics
        );
        let vtable_instance_refs = backend_lowering
            .program
            .modules
            .iter()
            .flat_map(|module| &module.trait_object_vtables)
            .flat_map(|vtable| &vtable.entries)
            .filter_map(|entry| match &entry.function {
                nia_backend_ir::BackendTraitObjectVtableFunction::FunctionInstance {
                    def_id,
                    arg_module_id,
                    self_arg,
                    args,
                    const_args,
                } => Some((
                    *def_id,
                    *arg_module_id,
                    *self_arg,
                    args.clone(),
                    const_args.clone(),
                )),
                nia_backend_ir::BackendTraitObjectVtableFunction::Function(_) => None,
            })
            .collect::<Vec<_>>();
        assert!(
            !vtable_instance_refs.is_empty(),
            "trait object vtable should reference a default method instance"
        );
        for (def_id, arg_module_id, self_arg, args, const_args) in vtable_instance_refs {
            let matches = backend_lowering
                .program
                .modules
                .iter()
                .flat_map(|module| {
                    module
                        .function_instances
                        .iter()
                        .map(move |instance| (module, instance))
                })
                .filter(|(_, instance)| {
                    backend_function_instance_matches_vtable_ref(
                        &codegen.type_store,
                        VtableFunctionInstanceRef {
                            def_id,
                            arg_module_id,
                            self_arg,
                            args: &args,
                            const_args: &const_args,
                        },
                        instance,
                    )
                })
                .count();
            assert_eq!(
                matches, 1,
                "expected one lowered vtable function instance for {def_id:?}"
            );
        }
        let helper = backend_lowering
            .program
            .modules
            .iter()
            .find(|module| module.id == module1_id)
            .expect("trait owner module should be backend-lowered")
            .functions
            .iter()
            .find(|function| function.name == sym("helper"))
            .expect("default method helper should be materialized");
        assert!(
            backend_lowering
                .optimization_report
                .changed_passes
                .iter()
                .any(|change| matches!(
                    change,
                    nia_backend_lower::BackendOptimizationChange::Function {
                        module_id,
                        function,
                        pass: "inline-leaf-functions",
                        is_instance: false,
                        ..
                    } if *module_id == module1_id && *function == helper.def_id
                )),
            "the vtable-induced default instance should be finalized after closure"
        );
    }

    #[test]
    fn executable_backend_lowering_closes_vtables_from_generic_function_instances() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module dispatch;
using entry::dispatch;

fn main() i32 {
    let mut page = dispatch::Page::init();
    dispatch::call[dispatch::Page](&mut page)
}
"#,
        );
        let entry_id = fixture.entry_id();
        let dispatch_id = fixture.add_child(
            entry_id,
            "dispatch",
            "dispatch.nia",
            r#"
pub trait Allocator {
    fn alloc(&mut self) i32;

    fn remap(&mut self) i32 {
        self.alloc()
    }
}

pub struct Page {}

extend Page {
    pub fn init() Page {
        {}
    }
}

extend Page : Allocator {
    fn alloc(&mut self) i32 {
        _ = self;
        7
    }
}

pub fn call[T](value: &mut T) i32
where T: Allocator
{
    let allocator: &mut Allocator = value;
    allocator.remap()
}
"#,
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let backend_lowering = db.expect_get(BackendLoweringQuery);
        assert!(
            backend_lowering.diagnostics.is_empty(),
            "backend lowering should not report diagnostics: {:?}",
            backend_lowering.diagnostics
        );
        let dispatch_module = backend_lowering
            .semantic
            .program
            .modules
            .iter()
            .find(|module| module.id == dispatch_id)
            .expect("generic function owner module should be backend-lowered");
        assert_eq!(
            dispatch_module
                .function_instances
                .iter()
                .filter(|instance| instance.name == sym("call"))
                .count(),
            1,
            "the concrete generic function should be materialized once"
        );
        assert_eq!(
            dispatch_module.trait_object_vtables.len(),
            1,
            "the substituted generic body should discover one trait-object vtable"
        );
        let vtable_instance_refs = dispatch_module
            .trait_object_vtables
            .iter()
            .flat_map(|vtable| &vtable.entries)
            .filter_map(|entry| match &entry.function {
                nia_backend_ir::BackendTraitObjectVtableFunction::FunctionInstance {
                    def_id,
                    arg_module_id,
                    self_arg,
                    args,
                    const_args,
                } => Some(VtableFunctionInstanceRef {
                    def_id: *def_id,
                    arg_module_id: *arg_module_id,
                    self_arg: *self_arg,
                    args,
                    const_args,
                }),
                nia_backend_ir::BackendTraitObjectVtableFunction::Function(_) => None,
            })
            .collect::<Vec<_>>();
        assert!(
            !vtable_instance_refs.is_empty(),
            "the vtable should reference the default method instance"
        );
        for vtable_ref in vtable_instance_refs {
            assert_eq!(
                dispatch_module
                    .function_instances
                    .iter()
                    .filter(|instance| backend_function_instance_matches_vtable_ref(
                        &db.context().type_store,
                        VtableFunctionInstanceRef {
                            def_id: vtable_ref.def_id,
                            arg_module_id: vtable_ref.arg_module_id,
                            self_arg: vtable_ref.self_arg,
                            args: vtable_ref.args,
                            const_args: vtable_ref.const_args,
                        },
                        instance,
                    ))
                    .count(),
                1,
                "each vtable method instance should be materialized once"
            );
        }
    }

    #[test]
    fn executable_backend_lowering_assigns_repeated_vtable_to_one_stable_owner() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module common;
module left;
module right;
using entry::left;
using entry::right;

fn main() i32 {
    left::read() + right::read()
}
"#,
        );
        let entry_id = fixture.entry_id();
        fixture.add_child(
            entry_id,
            "common",
            "common.nia",
            r#"
pub trait Value {
    fn read(& self) i32;
}

pub struct Cell {}

extend Cell : Value {
    fn read(& self) i32 {
        _ = self;
        7
    }
}
"#,
        );
        let left_id = fixture.add_child(
            entry_id,
            "left",
            "left.nia",
            r#"
using entry::common;

pub fn read() i32 {
    let cell: common::Cell = {};
    let value: &common::Value = &cell;
    value.read()
}
"#,
        );
        let right_id = fixture.add_child(
            entry_id,
            "right",
            "right.nia",
            r#"
using entry::common;

pub fn read() i32 {
    let cell: common::Cell = {};
    let value: &common::Value = &cell;
    value.read()
}
"#,
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;

        let backend = query_db(loaded).expect_get(BackendLoweringQuery);
        assert!(backend.diagnostics.is_empty(), "{:?}", backend.diagnostics);
        let owners = backend
            .semantic
            .program
            .modules
            .iter()
            .filter(|module| !module.trait_object_vtables.is_empty())
            .map(|module| module.id)
            .collect::<Vec<_>>();

        assert_eq!(owners, vec![left_id]);
        assert_ne!(left_id, right_id);
    }

    #[test]
    fn executable_backend_lowering_closes_cross_module_generic_local_static_instances() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module slots;
using entry::slots;

fn main() i32 {
    let mut left = slots::slot[i32]();
    let mut right = slots::slot[u64]();
    _ = left;
    _ = right;
    0
}
"#,
        );
        let entry_id = fixture.entry_id();
        let slots_id = fixture.add_child(
            entry_id,
            "slots",
            "slots.nia",
            r#"
pub fn slot[T]() &mut T {
    static mut item: T;
    &mut item
}
"#,
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let backend_lowering = db.expect_get(BackendLoweringQuery);
        assert!(
            backend_lowering.diagnostics.is_empty(),
            "backend lowering should not report diagnostics: {:?}",
            backend_lowering.diagnostics
        );
        let slots_module = backend_lowering
            .semantic
            .program
            .modules
            .iter()
            .find(|module| module.id == slots_id)
            .expect("generic function owner module should be backend-lowered");
        let item_instances = slots_module
            .global_instances
            .iter()
            .filter(|instance| instance.name == sym("item"))
            .collect::<Vec<_>>();

        assert_eq!(item_instances.len(), 2);
        assert!(item_instances.iter().any(|instance| matches!(
            db.context().type_store.get(instance.ty),
            Some(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I32))
        )));
        assert!(item_instances.iter().any(|instance| matches!(
            db.context().type_store.get(instance.ty),
            Some(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::U64))
        )));
    }

    #[test]
    fn executable_checked_modules_keep_type_owner_modules_type_only() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module types;
using entry::types;

fn main(value: types::Used) i32 {
    value.value
}
"#,
        );
        let entry_id = fixture.entry_id();
        let types_id = fixture.add_child(
            entry_id,
            "types",
            "types.nia",
            r#"
pub struct Used {
    value: i32,
}

pub fn unused_bad() i32 {
    missing_symbol
}
"#,
        );
        let types_description = format!("{types_id:?}");
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let type_module = modules
            .iter()
            .find(|module| module.id == types_id)
            .expect("type owner module should be present for backend type lookup");
        assert!(
            type_module.executable_type_only,
            "type owner module should not be treated as an executable body module"
        );
        assert!(
            type_module.body_ir.function_bodies.is_empty(),
            "type owner module should not retain or check function bodies"
        );

        let trace = db.query_trace();
        assert!(
            !trace.queries.iter().any(|query| {
                query.frame.name == "executable_body_check"
                    && query.frame.description.contains(&types_description)
                    && query.stats.executions > 0
            }),
            "type owner module should not be executable-body-checked: {:?}",
            trace
                .queries
                .iter()
                .filter(|query| query.frame.name == "executable_body_check")
                .collect::<Vec<_>>()
        );
        assert!(
            trace.queries.iter().any(|query| {
                query.frame.name == "signature_type_lowering"
                    && query.frame.description.contains(&types_description)
                    && query.frame.description.contains("Types")
                    && query.stats.executions > 0
            }),
            "type-only module should use signature type lowering: {:?}",
            trace
                .queries
                .iter()
                .filter(|query| query.frame.description.contains(&types_description))
                .collect::<Vec<_>>()
        );
        for full_query in ["type_resolution", "type_lowering", "value_resolution"] {
            assert!(
                !trace.queries.iter().any(|query| {
                    query.frame.name == full_query
                        && query.frame.description.contains(&types_description)
                        && query.stats.executions > 0
                }),
                "type-only module should not execute {full_query}: {:?}",
                trace
                    .queries
                    .iter()
                    .filter(|query| query.frame.name == full_query)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn executable_type_only_modules_keep_signature_const_enum_values() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module types;
using entry::types;

fn main(value: types::Mode) i32 {
    0
}
"#,
        );
        let entry_id = fixture.entry_id();
        let types_id = fixture.add_child(
            entry_id,
            "types",
            "types.nia",
            r#"
pub enum Mode: i32 {
    A = 1,
    B = 1 + 2,
}

pub fn unused_bad() i32 {
    missing_symbol
}
"#,
        );
        let types_description = format!("{types_id:?}");
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let type_module = modules
            .iter()
            .find(|module| module.id == types_id)
            .expect("type owner module should be present for backend type lookup");
        assert!(
            type_module.executable_type_only,
            "enum owner module should stay type-only"
        );
        let b = type_module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::EnumVariant && def.name == sym("B"))
                    .then_some(def_id)
            })
            .expect("enum variant B");
        assert!(
            matches!(
                type_module.const_eval.enum_values.get(&b),
                Some(nia_const_check::ConstValue::Int(value)) if value.bits() == 3
            ),
            "type-only signature const should evaluate enum discriminants: {:?}",
            type_module.const_eval.enum_values
        );

        let trace = db.query_trace();
        for full_query in ["type_resolution", "type_lowering", "value_resolution"] {
            assert!(
                !trace.queries.iter().any(|query| {
                    query.frame.name == full_query
                        && query.frame.description.contains(&types_description)
                        && query.stats.executions > 0
                }),
                "type-only enum module should not execute {full_query}: {:?}",
                trace
                    .queries
                    .iter()
                    .filter(|query| query.frame.name == full_query)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn executable_type_only_modules_keep_signature_const_array_lengths() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module types;
using entry::types;

fn main(value: types::Packet) i32 {
    0
}
"#,
        );
        let entry_id = fixture.entry_id();
        let types_id = fixture.add_child(
            entry_id,
            "types",
            "types.nia",
            r#"
const N: usize = 4;

pub struct Packet {
    data: [N]u8,
}

pub fn unused_bad() i32 {
    missing_symbol
}
"#,
        );
        let types_description = format!("{types_id:?}");
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let type_module = modules
            .iter()
            .find(|module| module.id == types_id)
            .expect("type owner module should be present for backend type lookup");
        assert!(
            type_module.executable_type_only,
            "array owner module should stay type-only"
        );
        assert!(
            type_module
                .const_eval
                .array_lengths
                .values()
                .any(|len| *len == 4),
            "type-only signature const should evaluate array length constants: {:?}",
            type_module.const_eval.array_lengths
        );

        let trace = db.query_trace();
        for full_query in ["type_resolution", "type_lowering", "value_resolution"] {
            assert!(
                !trace.queries.iter().any(|query| {
                    query.frame.name == full_query
                        && query.frame.description.contains(&types_description)
                        && query.stats.executions > 0
                }),
                "type-only array module should not execute {full_query}: {:?}",
                trace
                    .queries
                    .iter()
                    .filter(|query| query.frame.name == full_query)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn executable_checked_modules_do_not_body_check_modules_for_generic_metadata_only() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module helper;
using entry::helper;

fn main() i32 {
    helper::id[i32](1)
}
"#,
        );
        let entry_id = fixture.entry_id();
        let helper_id = fixture.add_child(
            entry_id,
            "helper",
            "helper.nia",
            r#"
pub fn id[T](value: T) T {
    value
}

fn unused_bad() i32 {
    missing_symbol
}
"#,
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let helper_module = modules
            .iter()
            .find(|module| module.id == helper_id)
            .expect("called generic function owner should be executable-reachable");
        let unused_bad = helper_module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Function && def.name == sym("unused_bad"))
                    .then_some(GlobalDefId {
                        module_id: helper_id,
                        def_id,
                    })
            })
            .expect("unused function");

        assert!(
            helper_module.body_diagnostics.is_empty(),
            "unused function in a generic callee module should not be body-checked: {:?}",
            helper_module.body_diagnostics
        );
        assert!(
            !helper_module
                .body_ir
                .function_bodies
                .contains_key(&unused_bad),
            "reachable generic metadata should not retain unrelated function bodies"
        );
    }

    #[test]
    fn executable_checked_modules_include_reachable_global_initializers() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
static used: i32 = 1;

fn main() i32 {
    used
}
"#,
        );
        let module_id = fixture.entry_id();
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module should be executable-reachable");
        let used = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Global && def.name == sym("used"))
                    .then_some(GlobalDefId { module_id, def_id })
            })
            .expect("used global");

        assert!(
            module.body_ir.global_inits.contains_key(&used),
            "reachable global initializers must be retained for executable codegen"
        );
    }

    #[test]
    fn executable_checked_modules_include_reachable_local_static_initializers() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
fn option_arg() &u8 {
    static text = b"-O2\0";
    &text[0]
}

fn main() i32 {
    _ = option_arg();
    0
}
"#,
        );
        let module_id = fixture.entry_id();
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module should be executable-reachable");
        let text = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Global && def.name == sym("text"))
                    .then_some(GlobalDefId { module_id, def_id })
            })
            .expect("local static global");

        assert!(
            module.body_ir.global_inits.contains_key(&text),
            "reachable local static initializers must be retained for executable codegen"
        );
    }

    #[test]
    fn executable_checked_modules_include_reachable_extension_method_local_static_initializers() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
enum Mode: i32 {
    O2 = 2,
}

extend Mode {
    fn argv(self) &u8 {
        static o2 = b"-O2\0";
        switch self {
            Mode::O2 => &o2[0],
            _ => &o2[0],
        }
    }
}

fn main() i32 {
    _ = Mode::O2.argv();
    0
}
"#,
        );
        let module_id = fixture.entry_id();
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module should be executable-reachable");
        let o2 = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Global && def.name == sym("o2"))
                    .then_some(GlobalDefId { module_id, def_id })
            })
            .expect("local static global");

        assert!(
            module.body_ir.global_inits.contains_key(&o2),
            "reachable extension method local static initializers must be retained for executable codegen"
        );
    }

    #[test]
    fn executable_checked_modules_include_cross_module_extension_method_local_static_initializers()
    {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
using helper::Mode;

fn main() i32 {
    _ = Mode::O2.argv();
    0
}
"#,
        );
        let entry_id = fixture.entry_id();
        let helper_id = fixture.add_child(
            entry_id,
            "helper",
            "helper.nia",
            r#"
pub enum Mode: i32 {
    O2 = 2,
}

extend Mode {
    pub fn argv(self) &u8 {
        static o2 = b"-O2\0";
        switch self {
            Mode::O2 => &o2[0],
            _ => &o2[0],
        }
    }
}
"#,
        );
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == helper_id)
            .expect("helper module should be executable-reachable");
        let o2 = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Global && def.name == sym("o2")).then_some(
                    GlobalDefId {
                        module_id: helper_id,
                        def_id,
                    },
                )
            })
            .expect("local static global");

        assert!(
            module.body_ir.global_inits.contains_key(&o2),
            "reachable cross-module extension method local static initializers must be retained for executable codegen"
        );
        assert!(
            module
                .executable_reachable_globals
                .as_ref()
                .is_some_and(|globals| globals.contains(&o2)),
            "reachable local static should be recorded in executable_reachable_globals: {:?}",
            module.executable_reachable_globals
        );

        let backend = db.expect_get(BackendLoweringQuery);
        let backend_module = backend
            .semantic
            .program
            .modules
            .iter()
            .find(|module| module.id == helper_id)
            .expect("helper backend module");
        assert!(
            backend_module
                .globals
                .iter()
                .any(|global| global.def_id == o2 && global.init.is_some()),
            "reachable cross-module extension method local static must lower as a backend global"
        );
    }

    #[test]
    fn executable_checked_modules_do_not_flow_check_unreachable_functions() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
fn unused() i32 {
}

fn main() i32 {
    0
}
"#,
        );
        let module_id = fixture.entry_id();
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module should be executable-reachable");

        assert!(
            module.flow_check.diagnostics.is_empty(),
            "unreachable function flow diagnostics should not block executable checking: {:?}",
            module.flow_check.diagnostics
        );
    }

    #[test]
    fn executable_checked_modules_do_not_body_check_unreachable_loaded_modules() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
pub module unused;

fn main() i32 {
    0
}
"#,
        );
        let entry_id = fixture.entry_id();
        let unused_id = fixture.add_child(
            entry_id,
            "unused",
            "unused.nia",
            r#"
pub fn expensive_or_invalid() i32 {
    missing_symbol
}
"#,
        );
        let unused_description = format!("{unused_id:?}");
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.expect_get(ExecutableCheckedModulesQuery);
        let trace = db.query_trace();

        assert!(
            modules.iter().all(|module| module.id != unused_id),
            "unreachable module should not be kept for executable codegen"
        );
        assert!(
            !trace.queries.iter().any(|query| {
                query.frame.name == "body_check"
                    && query.frame.description.contains(&unused_description)
                    && query.stats.executions > 0
            }),
            "unreachable module should not be body-checked: {:?}",
            trace
                .queries
                .iter()
                .filter(|query| query.frame.name == "body_check")
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn body_check_uses_const_semantic_modules_not_ast_module_map() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "const N: usize = 4; fn main() i32 { let mut values: [N]i32 = [0; N]; values.len() as i32 }",
        );
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let _ = db.expect_get(BodyCheckQuery(module_id));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "const_module"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "const_values"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "const_array_lengths"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check"
                && dependency.to.name == "full_active_module_item_tree"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "const"
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
                .any(|dependency| dependency.to.name == "program_modules_by_id")
        );
        assert!(
            !trace
                .dependencies
                .iter()
                .any(|dependency| dependency.to.name == "program_item_signatures")
        );
    }

    #[test]
    fn direct_module_defs_invalidation_stops_at_snapshot_boundary() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "pub struct S { value: i32 } fn main() i32 { 0 }",
        );
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let _ = db.expect_get(TypeResolutionQuery(module_id));
        let invalidation = db.invalidate(ModuleDefsQuery(module_id));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(invalidated.contains(&"module_defs"), "{invalidated:?}");
        assert!(!invalidated.contains(&"public_surfaces"), "{invalidated:?}");
        assert!(
            !invalidated.contains(&"public_using_scopes"),
            "{invalidated:?}"
        );
        assert!(
            !invalidated.contains(&"module_using_scope"),
            "{invalidated:?}"
        );
        assert!(!invalidated.contains(&"type_resolution"), "{invalidated:?}");

        let _ = db.expect_get(TypeResolutionQuery(module_id));
    }

    #[test]
    fn invalidates_module_defs_after_item_tree_changes() {
        let fixture = LoadedProgramFixture::new("main.nia", "pub struct S { value: i32 }");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let _ = db.expect_get(ModuleDefsQuery(module_id));
        let invalidation = db.invalidate(ModuleItemTreeInputQuery(module_id));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(
            invalidated.contains(&"module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"module_item_tree"), "{invalidated:?}");
        assert!(
            invalidated.contains(&"active_module_item_tree"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"module_defs"), "{invalidated:?}");
    }
}
