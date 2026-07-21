// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    ActiveModuleItemTreeFactKind, CheckedModule, CheckedProgram, CodegenProgram, LoadedModule,
    LoadedProgram, ProgramDiagnostic, RuntimeModel, TimingMode, module_diagnostics,
};
use nia_backend_lower::BackendLowerModuleInput;
use nia_const_check::{ConstCheck, ConstModuleLowering};
use nia_defs::{
    DefCollection, ModulePublicSurface, ModuleUsingScope, PublicSurfaceLookup, PublicSurfaces,
    UsingScopeLookup,
};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId};
#[cfg(test)]
use nia_imports::ModuleGraph;
use nia_imports::{ModuleGraphLookup, ModuleGraphSnapshot, ModuleNode, StableModuleKey};
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
    collect_extension_method_index_for_module, collect_extension_methods_for_module,
    collect_nominal_extension_providers_for_module, visible_extensions_for_module,
    visible_trait_impls_for_module,
};
use nia_public_surface::{
    TypeExposureIndex, compute_exported_public_surfaces_with_symbols,
    compute_using_scopes_from_surfaces_with_symbols,
};
use nia_query::{
    QueryDb, QueryError, QueryFingerprint, QueryFingerprintBuilder, QueryFingerprintPolicy,
    QueryFrame, QueryKey, QueryTrace,
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
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
};

mod backend_lowering;
mod base;
mod checked;
mod checks;
mod diagnostics;
mod executable;
mod extension_provider_queries;
#[cfg(test)]
mod program;
mod program_signature_queries;
mod providers;
mod resolve;
mod types;

use backend_lowering::*;
use base::*;
use checked::*;
use checks::*;
use diagnostics::*;
use executable::*;
use extension_provider_queries::*;
#[cfg(test)]
use program::*;
use program_signature_queries::*;
use providers::*;
use resolve::*;
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
        BodyCheckQuery,
        CheckedModuleIdsQuery,
        CheckedModuleQuery,
        CheckedProgramQuery,
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
        ExecutableCheckedModuleSetQuery,
        ExecutableFactEpochQuery,
        ExecutableProviderDemandsQuery,
        ExecutableRootModulesQuery,
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
        LoweredFunctionBodiesQuery,
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
        ModuleSourceVersionQuery,
        ModuleUsingScopeQuery,
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
    #[cfg(test)]
    register!(
        BackendLoweringQuery,
        ExecutableCheckedModulesQuery,
        MonomorphizationQuery,
    );
    registry
}

#[derive(Clone)]
pub struct CompileRequest {
    loader_facts: Arc<dyn crate::LoaderFactProvider>,
    pub provider_fact_revision: crate::ProviderFactRevision,
    pub optimization: NiaOptimizationLevel,
    pub timings: TimingMode,
    pub provider_changes: HashSet<crate::ProviderDemand>,
}

impl CompileRequest {
    pub fn new(loader_facts: impl crate::LoaderFactProvider + 'static) -> Self {
        let loader_facts: Arc<dyn crate::LoaderFactProvider> = Arc::new(loader_facts);
        let provider_fact_revision = loader_facts.provider_fact_revision();
        Self {
            loader_facts,
            provider_fact_revision,
            optimization: NiaOptimizationLevel::default(),
            timings: TimingMode::Off,
            provider_changes: HashSet::new(),
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

    pub fn with_provider_changes(
        mut self,
        provider_changes: impl IntoIterator<Item = crate::ProviderDemand>,
    ) -> Self {
        self.provider_changes = provider_changes.into_iter().collect();
        self
    }

    pub fn with_provider_fact_revision(
        mut self,
        provider_fact_revision: crate::ProviderFactRevision,
    ) -> Self {
        self.provider_fact_revision = provider_fact_revision;
        self
    }
}

#[derive(Clone)]
pub struct CompilerDatabase {
    db: QueryDb<CompilerContext>,
    inputs: Arc<RwLock<CompilerInputs>>,
    loader_facts: Arc<RwLock<Arc<dyn crate::LoaderFactProvider>>>,
}

impl CompilerDatabase {
    pub fn new(request: CompileRequest) -> Self {
        compiler_database_with_providers(request, CompilerQueryProviders::default())
    }

    pub fn new_in_session(request: CompileRequest, session: nia_query::QuerySession) -> Self {
        compiler_database_with_providers_in_session(
            request,
            CompilerQueryProviders::default(),
            session,
        )
    }

    pub fn query_session(&self) -> nia_query::QuerySession {
        self.db.session()
    }

    pub fn check_program(&self) -> CheckedProgram {
        #[cfg(test)]
        let _permit = nia_test_support::compiler_permit();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.db.try_get(CheckedProgramQuery)
        })) {
            Ok(Ok(checked)) => Arc::unwrap_or_clone(checked),
            Ok(Err(err)) => checked_program_from_query_error(
                self.current_graph(),
                self.current_optimization(),
                err,
            ),
            Err(payload) => match payload.downcast::<QueryError>() {
                Ok(err) => checked_program_from_query_error(
                    self.current_graph(),
                    self.current_optimization(),
                    *err,
                ),
                Err(payload) => std::panic::resume_unwind(payload),
            },
        }
    }

    pub fn entry_check_program(&self) -> CheckedProgram {
        #[cfg(test)]
        let _permit = nia_test_support::compiler_permit();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.db.try_get(EntryCheckedProgramQuery)
        })) {
            Ok(Ok(checked)) => Arc::unwrap_or_clone(checked),
            Ok(Err(err)) => checked_program_from_query_error(
                self.current_graph(),
                self.current_optimization(),
                err,
            ),
            Err(payload) => match payload.downcast::<QueryError>() {
                Ok(err) => checked_program_from_query_error(
                    self.current_graph(),
                    self.current_optimization(),
                    *err,
                ),
                Err(payload) => std::panic::resume_unwind(payload),
            },
        }
    }

    pub fn executable_provider_demands(&self) -> Vec<crate::ProviderDemand> {
        #[cfg(test)]
        let _permit = nia_test_support::compiler_permit();
        self.db
            .try_get(ExecutableProviderDemandsQuery)
            .map(Arc::unwrap_or_clone)
            .unwrap_or_default()
    }

    pub fn provider_fact_revision(&self) -> crate::ProviderFactRevision {
        *self.db.get(ProviderFactRevisionQuery)
    }

    pub fn codegen_program(&self) -> CodegenProgram {
        #[cfg(test)]
        let _permit = nia_test_support::compiler_permit();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.db.try_get(CodegenProgramQuery)
        })) {
            Ok(Ok(codegen)) => Arc::unwrap_or_clone(codegen),
            Ok(Err(err)) => codegen_program_from_query_error(
                Arc::clone(&self.db.context().type_store),
                self.current_graph(),
                self.current_optimization(),
                err,
            ),
            Err(payload) => match payload.downcast::<QueryError>() {
                Ok(err) => codegen_program_from_query_error(
                    Arc::clone(&self.db.context().type_store),
                    self.current_graph(),
                    self.current_optimization(),
                    *err,
                ),
                Err(payload) => std::panic::resume_unwind(payload),
            },
        }
    }

    pub fn update(&self, request: CompileRequest) -> CompilerInvalidation {
        let loader_facts = Arc::clone(&request.loader_facts);
        let mut new_inputs = CompilerInputs::new(request);
        let update = {
            let mut inputs = self.inputs.write().expect("compiler input lock poisoned");
            match CompilerInputDiff::try_between(&inputs, &new_inputs) {
                Ok(diff) => {
                    new_inputs.merge_provider_fact_worklist(&inputs);
                    new_inputs.merge_body_activation_worklist(&inputs, &diff);
                    new_inputs.advance_executable_fact_epoch(&inputs, &diff);
                    *inputs = new_inputs;
                    Ok(diff)
                }
                Err(error) => Err(error),
            }
        };
        let diff = match update {
            Ok(diff) => diff,
            Err(error) => panic!("Nia ICE: {error}"),
        };
        *self
            .loader_facts
            .write()
            .expect("compiler loader fact lock poisoned") = loader_facts;
        self.invalidate_inputs(diff)
    }

    pub fn query_trace(&self) -> QueryTrace {
        self.db.query_trace()
    }

    fn current_graph(&self) -> ModuleGraphSnapshot {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .graph
            .clone()
    }

    fn current_optimization(&self) -> OptimizationPolicy {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .optimization
    }

    fn invalidate_inputs(&self, diff: CompilerInputDiff) -> CompilerInvalidation {
        let mut invalidation = CompilerInvalidation::default();
        if diff.graph_entry_changed {
            let entry = self.db.context().module_graph_entry_key();
            invalidation.extend(self.db.validate_input(ModuleGraphEntryQuery, &entry));
        }
        for module_id in diff.changed_graph_paths {
            let path = self.db.context().module_graph_path(module_id);
            invalidation.extend(
                self.db
                    .validate_input(ModuleGraphPathQuery(module_id), &path),
            );
        }
        for module_id in diff.changed_graph_parents {
            let parent = self.db.context().module_graph_parent_key(module_id);
            invalidation.extend(
                self.db
                    .validate_input(ModuleGraphParentQuery(module_id), &parent),
            );
        }
        for (module_id, name) in diff.changed_graph_children {
            let child = self.db.context().module_graph_child_key(module_id, &name);
            invalidation.extend(
                self.db
                    .validate_input(ModuleGraphChildQuery(module_id, name), &child),
            );
        }
        for package in diff.changed_package_roots {
            let root = self.db.context().module_package_root_key(&package);
            invalidation.extend(
                self.db
                    .validate_input(ModulePackageRootQuery(package), &root),
            );
        }
        if diff.graph_changed {
            invalidation.extend(self.db.invalidate(ModuleGraphQuery));
        }
        if diff.executable_roots_changed {
            invalidation.extend(self.db.invalidate(ExecutableRootModulesQuery));
        }
        if diff.loaded_modules_changed {
            let loaded_modules =
                stable_module_sequence(&self.db, self.db.context().loaded_modules());
            invalidation.extend(self.db.validate_input(LoadedModulesQuery, &loaded_modules));
        }
        if diff.loaded_diagnostics_changed {
            invalidation.extend(self.db.invalidate(ProgramLoadDiagnosticsQuery));
        }
        if diff.target_changed {
            invalidation.extend(self.db.invalidate(CompilerTargetQuery));
        }
        if diff.runtime_changed {
            invalidation.extend(self.db.invalidate(CompilerRuntimeQuery));
        }
        let provider_worklist = self.db.context().provider_fact_worklist();
        invalidation.extend(
            self.db
                .validate_input(ProviderFactWorklistQuery, &provider_worklist),
        );
        let body_activation_worklist = self.db.context().body_activation_worklist();
        invalidation.extend(
            self.db
                .validate_input(BodyActivationWorklistQuery, &body_activation_worklist),
        );
        let executable_fact_epoch = self.db.context().executable_fact_epoch();
        invalidation.extend(
            self.db
                .validate_input(ExecutableFactEpochQuery, &executable_fact_epoch),
        );
        if diff.optimization_changed {
            invalidation.extend(self.db.invalidate(CompilerOptimizationQuery));
        }
        for module in diff.changed_modules {
            for module_id in module.ids {
                if module.path {
                    if let Some(path) = self.db.context().module_path_if_loaded(module_id) {
                        invalidation
                            .extend(self.db.validate_input(ModulePathQuery(module_id), &path));
                    } else {
                        invalidation.extend(self.db.invalidate(ModulePathQuery(module_id)));
                    }
                }
                if module.source_version {
                    if let Some(source_version) =
                        self.db.context().module_source_version_if_loaded(module_id)
                    {
                        invalidation.extend(
                            self.db.validate_input(
                                ModuleSourceVersionQuery(module_id),
                                &source_version,
                            ),
                        );
                    } else {
                        invalidation
                            .extend(self.db.invalidate(ModuleSourceVersionQuery(module_id)));
                    }
                }
                if module.full_item_tree {
                    invalidation
                        .extend(self.db.invalidate(FullModuleItemTreeInputQuery(module_id)));
                }
                if module.origins {
                    invalidation.extend(self.db.invalidate(ModuleOriginsQuery(module_id)));
                }
                if module.parse_errors {
                    invalidation.extend(self.db.invalidate(ModuleParseErrorsQuery(module_id)));
                }
                if module.item_tree {
                    invalidation.extend(self.db.invalidate(ModuleItemTreeInputQuery(module_id)));
                }
                if module.declaration_item_tree {
                    invalidation.extend(
                        self.db
                            .invalidate(DeclarationModuleItemTreeInputQuery(module_id)),
                    );
                }
                if module.active_item_tree {
                    invalidation.extend(
                        self.db
                            .invalidate(ActiveModuleItemTreeInputQuery(module_id)),
                    );
                }
                if module.declaration_active_item_tree {
                    invalidation.extend(
                        self.db
                            .invalidate(DeclarationActiveModuleItemTreeInputQuery(module_id)),
                    );
                }
                if module.signature_function_items {
                    invalidation.extend(self.db.invalidate(SignatureItemTreeQuery(
                        module_id,
                        nia_item_tree::SignatureItemSet::Functions,
                    )));
                }
                if module.signature_extension_function_items {
                    invalidation.extend(self.db.invalidate(SignatureItemTreeQuery(
                        module_id,
                        nia_item_tree::SignatureItemSet::ExtensionFunctions,
                    )));
                }
                if module.signature_value_items {
                    invalidation.extend(self.db.invalidate(SignatureItemTreeQuery(
                        module_id,
                        nia_item_tree::SignatureItemSet::Values,
                    )));
                }
                if module.signature_type_items {
                    invalidation.extend(self.db.invalidate(SignatureItemTreeQuery(
                        module_id,
                        nia_item_tree::SignatureItemSet::Types,
                    )));
                }
                if module.provider_summary {
                    if let Some(summary) = self
                        .db
                        .context()
                        .loaded_module(module_id)
                        .map(|module| module.provider_summary)
                    {
                        invalidation.extend(
                            self.db
                                .validate_input(ExtensionProviderSummaryQuery(module_id), &summary),
                        );
                    } else {
                        invalidation
                            .extend(self.db.invalidate(ExtensionProviderSummaryQuery(module_id)));
                    }
                }
                if module.signature_trait_items {
                    invalidation.extend(self.db.invalidate(SignatureItemTreeQuery(
                        module_id,
                        nia_item_tree::SignatureItemSet::Traits,
                    )));
                }
                if module.signature_const_items {
                    invalidation.extend(self.db.invalidate(SignatureConstItemTreeQuery(module_id)));
                }
                if module.full_active_item_tree {
                    invalidation.extend(
                        self.db
                            .invalidate(FullActiveModuleItemTreeInputQuery(module_id)),
                    );
                }
            }
        }
        if invalidation
            .invalidated
            .iter()
            .any(|frame| frame.name == "executable_checked_module_set")
        {
            self.db.context().clear_executable_checked_module_sets();
        }
        invalidation
    }
}

impl std::fmt::Debug for CompilerDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        f.debug_struct("CompilerDatabase")
            .field("graph", &inputs.graph)
            .field("optimization", &inputs.optimization)
            .finish_non_exhaustive()
    }
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
    compiler_database_with_providers_in_session(request, providers, nia_query::QuerySession::new())
}

fn compiler_database_with_providers_in_session(
    request: CompileRequest,
    providers: CompilerQueryProviders,
    session: nia_query::QuerySession,
) -> CompilerDatabase {
    let timings = request.timings;
    let loader_facts = Arc::new(RwLock::new(Arc::clone(&request.loader_facts)));
    let inputs = Arc::new(RwLock::new(CompilerInputs::new(request)));
    let node_store = inputs
        .read()
        .expect("compiler input lock poisoned")
        .modules
        .first()
        .map(|module| module.origins.node_store().clone())
        .unwrap_or_default();
    let executable_checked_modules = Arc::new(RwLock::new(ExecutableCheckedModuleStore::default()));
    let executable_fact_session = Arc::new(std::sync::Mutex::new(ExecutableFactSession::default()));
    let type_store = Arc::new(nia_ty::TypeStore::new());
    let db = QueryDb::new_registered_with_timings_in_session(
        CompilerContext {
            inputs: inputs.clone(),
            loader_facts: loader_facts.clone(),
            providers,
            executable_checked_modules,
            executable_fact_session,
            type_store,
            node_store,
        },
        timings,
        compiler_query_registry(),
        session,
    );
    CompilerDatabase {
        db,
        inputs,
        loader_facts,
    }
}

fn checked_program_from_query_error(
    graph: ModuleGraphSnapshot,
    optimization: OptimizationPolicy,
    err: QueryError,
) -> CheckedProgram {
    CheckedProgram {
        graph,
        optimization,
        modules: Vec::new(),
        diagnostics: vec![ProgramDiagnostic {
            path: SourcePath::new("<query>"),
            diagnostic: query_error_diagnostic(err),
        }],
    }
}

fn codegen_program_from_query_error(
    type_store: Arc<nia_ty::TypeStore>,
    graph: ModuleGraphSnapshot,
    optimization: OptimizationPolicy,
    err: QueryError,
) -> CodegenProgram {
    CodegenProgram {
        type_store,
        graph,
        optimization,
        modules: Vec::new(),
        monomorphization: Arc::new(nia_monomorphize::Monomorphization {
            instances: Vec::new(),
            diagnostics: Vec::new(),
        }),
        backend_lowering: Arc::new(nia_backend_lower::BackendLowering {
            program: nia_backend_ir::BackendProgram {
                modules: Vec::new(),
            },
            optimization,
            optimization_report: nia_backend_lower::BackendOptimizationReport::default(),
            diagnostics: Vec::new(),
        }),
        diagnostics: vec![ProgramDiagnostic {
            path: SourcePath::new("<query>"),
            diagnostic: query_error_diagnostic(err),
        }],
    }
}

fn query_error_diagnostic(err: QueryError) -> Diagnostic {
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
    loader_facts: Arc<RwLock<Arc<dyn crate::LoaderFactProvider>>>,
    providers: CompilerQueryProviders,
    executable_checked_modules: Arc<RwLock<ExecutableCheckedModuleStore>>,
    executable_fact_session: Arc<std::sync::Mutex<ExecutableFactSession>>,
    type_store: Arc<nia_ty::TypeStore>,
    node_store: nia_node_id::NodeStore,
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
) -> StableModuleSequence {
    db.context().stable_module_sequence(module_ids)
}

fn resolve_stable_module_sequence_from_current_inputs(
    db: &QueryDb<CompilerContext>,
    sequence: &StableModuleSequence,
) -> Vec<ModuleId> {
    db.context().resolve_stable_module_sequence(sequence)
}

fn resolve_stable_module_sequence(
    db: &QueryDb<CompilerContext>,
    sequence: &StableModuleSequence,
) -> Vec<ModuleId> {
    let _graph = db.get(ModuleGraphQuery);
    db.context().resolve_stable_module_sequence(sequence)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExecutableCheckedModuleSetId(u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExecutableCheckedModuleSet {
    pub(super) id: ExecutableCheckedModuleSetId,
    pub(super) module_ids: Vec<ModuleId>,
}

#[derive(Default)]
struct ExecutableCheckedModuleStore {
    next_id: u64,
    sets: HashMap<ExecutableCheckedModuleSetId, ExecutableCheckedModuleSetData>,
}

struct ExecutableCheckedModuleSetData {
    module_ids: Vec<ModuleId>,
    modules: HashMap<ModuleId, Arc<CheckedModule>>,
}

#[derive(Debug, Clone)]
struct CompilerInputs {
    graph: ModuleGraphSnapshot,
    provider_fact_revision: crate::ProviderFactRevision,
    entry_module: ModuleId,
    runtime_root_modules: Vec<ModuleId>,
    modules: Vec<CompilerInputModule>,
    modules_by_id: HashMap<ModuleId, usize>,
    modules_by_source_identity: HashMap<SourceIdentity, usize>,
    diagnostics: Vec<ProgramDiagnostic>,
    target: TargetConfig,
    runtime: crate::RuntimeModel,
    optimization: OptimizationPolicy,
    timings: TimingMode,
    provider_worklist: Arc<HashSet<crate::ProviderDemand>>,
    body_activation_worklist: Arc<HashMap<StableModuleKey, ModuleId>>,
    executable_fact_epoch: ExecutableFactEpoch,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ExecutableFactEpoch(u64);

impl ExecutableFactEpoch {
    fn next(self) -> Self {
        Self(
            self.0
                .checked_add(1)
                .expect("executable fact epoch overflow"),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderFactWorklist {
    revision: crate::ProviderFactRevision,
    changes: Arc<HashSet<crate::ProviderDemand>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BodyActivationWorklist {
    modules: Arc<HashMap<StableModuleKey, ModuleId>>,
}

fn provider_fact_worklist_fingerprint(worklist: &ProviderFactWorklist) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.compiler.provider-fact-worklist.v1");
    builder.write_fingerprint(provider_fact_revision_fingerprint(worklist.revision));
    let mut changes = worklist
        .changes
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

fn body_activation_worklist_fingerprint(worklist: &BodyActivationWorklist) -> QueryFingerprint {
    let mut stable_modules = worklist.modules.keys().collect::<Vec<_>>();
    stable_modules.sort_unstable();
    let mut builder = QueryFingerprintBuilder::new("nia.compiler.body-activation-worklist.v1");
    builder.write_u64(stable_modules.len() as u64);
    for stable_module in stable_modules {
        builder.write_str(stable_module.source_identity().normalized_path());
    }
    builder.finish()
}

fn executable_fact_epoch_fingerprint(epoch: ExecutableFactEpoch) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.compiler.executable-fact-epoch.v1");
    builder.write_u64(epoch.0);
    builder.finish()
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

#[derive(Debug, Clone)]
struct CompilerInputModule {
    id: ModuleId,
    path: SourcePath,
    source_identity: SourceIdentity,
    source_version: SourceVersion,
    item_tree: Arc<ModuleItemTree>,
    active_item_tree: Arc<ActiveModuleItemTree>,
    provider_summary: nia_provider_summary::ProviderSummary,
    origins: NodeOriginTable,
    parse_errors: Vec<ParseError>,
}

impl CompilerInputModule {
    fn from_loaded(module: LoadedModule) -> Self {
        Self {
            id: module.id,
            path: module.path,
            source_identity: module.source_identity,
            source_version: module.source_version,
            item_tree: Arc::new(module.item_tree),
            active_item_tree: Arc::new(module.active_item_tree),
            provider_summary: module.provider_summary,
            origins: module.origins,
            parse_errors: module.parse_errors,
        }
    }
}

impl CompilerInputs {
    fn new(request: CompileRequest) -> Self {
        let loaded = request.loader_facts.loaded_program();
        validate_loaded_module_identities(&loaded);
        let graph = loaded.graph;
        let entry_module = graph.entry();
        let runtime_root_modules = graph
            .modules()
            .filter(|node| graph.is_executable_root_module(node.id))
            .map(|node| node.id)
            .collect();
        let target = loaded.target;
        let runtime = loaded.runtime;
        let diagnostics = loaded.diagnostics;
        let provider_worklist = Arc::new(request.provider_changes);
        let modules = loaded
            .modules
            .into_iter()
            .map(CompilerInputModule::from_loaded)
            .collect::<Vec<_>>();
        let modules_by_id = index_input_modules(&modules);
        let modules_by_source_identity = index_input_module_identities(&modules);
        Self {
            graph,
            provider_fact_revision: request.provider_fact_revision,
            entry_module,
            runtime_root_modules,
            modules,
            modules_by_id,
            modules_by_source_identity,
            diagnostics,
            target,
            runtime,
            optimization: request.optimization.policy(),
            timings: request.timings,
            provider_worklist,
            body_activation_worklist: Arc::new(HashMap::new()),
            executable_fact_epoch: ExecutableFactEpoch::default(),
        }
    }

    fn merge_provider_fact_worklist(&mut self, previous: &Self) {
        use crate::ProviderFactRevisionTransition::{Advanced, Unchanged};

        match self
            .provider_fact_revision
            .transition_from(previous.provider_fact_revision)
        {
            Unchanged => {
                debug_assert!(self.provider_worklist.is_empty());
                self.provider_worklist = Arc::clone(&previous.provider_worklist);
            }
            Advanced if !self.provider_worklist.is_empty() => {
                Arc::make_mut(&mut self.provider_worklist)
                    .extend(previous.provider_worklist.iter().cloned());
            }
            _ => {}
        }
    }

    fn merge_body_activation_worklist(&mut self, previous: &Self, diff: &CompilerInputDiff) {
        if diff.resets_executable_facts() {
            return;
        }
        let worklist = Arc::make_mut(&mut self.body_activation_worklist);
        worklist.extend(
            previous
                .body_activation_worklist
                .iter()
                .map(|(stable_key, module_id)| (stable_key.clone(), *module_id)),
        );
        for module_id in &diff.body_activated_modules {
            let stable_key = self.graph.stable_key(*module_id).unwrap_or_else(|| {
                panic!("Nia ICE: missing stable key for activated module {module_id:?}")
            });
            worklist.insert(stable_key.clone(), *module_id);
        }
    }

    fn advance_executable_fact_epoch(&mut self, previous: &Self, diff: &CompilerInputDiff) {
        self.executable_fact_epoch = if diff.resets_executable_facts() {
            previous.executable_fact_epoch.next()
        } else {
            previous.executable_fact_epoch
        };
    }
}

fn validate_loaded_module_identities(loaded: &LoadedProgram) {
    for module in &loaded.modules {
        let expected = module.path.identity();
        if module.source_identity != expected {
            panic!(
                "Nia ICE: loaded module {:?} has source identity `{}` but path `{}` implies `{}`",
                module.id,
                module.source_identity.normalized_path(),
                module.path.as_str(),
                expected.normalized_path()
            );
        }
    }
}

impl CompilerContext {
    fn loader_facts(&self) -> Arc<dyn crate::LoaderFactProvider> {
        Arc::clone(
            &self
                .loader_facts
                .read()
                .expect("compiler loader fact lock poisoned"),
        )
    }

    fn type_store(&self) -> &nia_ty::TypeStore {
        &self.type_store
    }

    fn node_store(&self) -> &nia_node_id::NodeStore {
        &self.node_store
    }

    fn take_executable_fact_session(&self) -> ExecutableFactSession {
        std::mem::take(
            &mut *self
                .executable_fact_session
                .lock()
                .expect("executable fact session lock poisoned"),
        )
    }

    fn store_executable_fact_session(&self, session: ExecutableFactSession) {
        *self
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned") = session;
    }

    fn executable_root_modules(&self) -> (ModuleId, Vec<ModuleId>) {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        (inputs.entry_module, inputs.runtime_root_modules.clone())
    }

    fn store_executable_checked_modules(
        &self,
        modules: Vec<CheckedModule>,
    ) -> ExecutableCheckedModuleSet {
        let module_ids = modules.iter().map(|module| module.id).collect::<Vec<_>>();
        let modules = modules
            .into_iter()
            .map(|module| (module.id, Arc::new(module)))
            .collect::<HashMap<_, _>>();
        let mut store = self
            .executable_checked_modules
            .write()
            .expect("executable checked module store lock poisoned");
        let id = ExecutableCheckedModuleSetId(store.next_id);
        store.next_id += 1;
        store.sets.insert(
            id,
            ExecutableCheckedModuleSetData {
                module_ids: module_ids.clone(),
                modules,
            },
        );
        ExecutableCheckedModuleSet { id, module_ids }
    }

    fn executable_checked_modules(
        &self,
        set: &ExecutableCheckedModuleSet,
    ) -> Vec<Arc<CheckedModule>> {
        let store = self
            .executable_checked_modules
            .read()
            .expect("executable checked module store lock poisoned");
        let data = store.sets.get(&set.id).unwrap_or_else(|| {
            panic!(
                "Nia ICE: missing executable checked module set {:?}",
                set.id
            )
        });
        data.module_ids
            .iter()
            .map(|module_id| {
                data.modules
                    .get(module_id)
                    .unwrap_or_else(|| {
                        panic!(
                            "Nia ICE: missing executable checked module {:?} in set {:?}",
                            module_id, set.id
                        )
                    })
                    .clone()
            })
            .collect()
    }

    fn executable_checked_module(
        &self,
        set: &ExecutableCheckedModuleSet,
        module_id: ModuleId,
    ) -> Arc<CheckedModule> {
        let store = self
            .executable_checked_modules
            .read()
            .expect("executable checked module store lock poisoned");
        let data = store.sets.get(&set.id).unwrap_or_else(|| {
            panic!(
                "Nia ICE: missing executable checked module set {:?}",
                set.id
            )
        });
        Arc::clone(data.modules.get(&module_id).unwrap_or_else(|| {
            panic!(
                "Nia ICE: missing executable checked module {:?} in set {:?}",
                module_id, set.id
            )
        }))
    }

    fn clear_executable_checked_module_sets(&self) {
        let mut store = self
            .executable_checked_modules
            .write()
            .expect("executable checked module store lock poisoned");
        store.sets.clear();
    }

    fn loaded_modules(&self) -> Vec<ModuleId> {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .modules
            .iter()
            .map(|module| module.id)
            .collect()
    }

    fn stable_module_sequence(
        &self,
        module_ids: impl IntoIterator<Item = ModuleId>,
    ) -> StableModuleSequence {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        StableModuleSequence::from_source_identities(module_ids.into_iter().map(|module_id| {
            inputs
                .loaded_module(module_id)
                .unwrap_or_else(|| {
                    panic!("Nia ICE: module {module_id:?} is not loaded in compiler inputs")
                })
                .source_identity
                .clone()
        }))
    }

    fn resolve_stable_module_sequence(&self, sequence: &StableModuleSequence) -> Vec<ModuleId> {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        sequence
            .keys
            .iter()
            .map(|key| {
                inputs
                    .loaded_module_by_source_identity(key.source_identity())
                    .unwrap_or_else(|| {
                        panic!(
                            "Nia ICE: stable loaded module `{}` is missing from compiler inputs",
                            key.source_identity().normalized_path()
                        )
                    })
                    .id
            })
            .collect()
    }

    fn loaded_module(&self, module_id: ModuleId) -> Option<CompilerInputModule> {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        inputs
            .modules_by_id
            .get(&module_id)
            .and_then(|index| inputs.modules.get(*index))
            .cloned()
    }

    fn module_path(&self, db: &QueryDb<CompilerContext>, module_id: ModuleId) -> SourcePath {
        self.loader_facts()
            .module_path(module_id)
            .unwrap_or_else(|| {
                db.invalid_input(
                    &ModulePathQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn module_path_if_loaded(&self, module_id: ModuleId) -> Option<SourcePath> {
        self.loader_facts().module_path(module_id)
    }

    fn module_source_version(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> SourceVersion {
        self.loader_facts()
            .module_source_version(module_id)
            .unwrap_or_else(|| {
                db.invalid_input(
                    &ModuleSourceVersionQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn module_source_version_if_loaded(&self, module_id: ModuleId) -> Option<SourceVersion> {
        self.loader_facts().module_source_version(module_id)
    }

    fn module_origins(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> NodeOriginTable {
        self.loader_facts()
            .module_origins(module_id)
            .unwrap_or_else(|| {
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
    ) -> Vec<ParseError> {
        self.loader_facts()
            .module_parse_errors(module_id)
            .unwrap_or_else(|| {
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
    ) -> ModuleItemTree {
        self.loader_facts()
            .module_item_tree(module_id)
            .unwrap_or_else(|| {
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
    ) -> ModuleItemTree {
        self.loader_facts()
            .module_item_tree(module_id)
            .unwrap_or_else(|| {
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
    ) -> ModuleItemTree {
        self.loader_facts()
            .module_item_tree(module_id)
            .unwrap_or_else(|| {
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
    ) -> ActiveModuleItemTree {
        self.loader_facts()
            .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::Full)
            .unwrap_or_else(|| {
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
    ) -> ActiveModuleItemTree {
        self.loader_facts()
            .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::Full)
            .unwrap_or_else(|| {
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
    ) -> ActiveModuleItemTree {
        self.loader_facts()
            .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::Full)
            .unwrap_or_else(|| {
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
    ) -> ActiveModuleItemTree {
        self.loader_facts()
            .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::Signature(set))
            .unwrap_or_else(|| {
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
    ) -> ActiveModuleItemTree {
        self.loader_facts()
            .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::ConstSignature)
            .unwrap_or_else(|| {
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
    ) -> nia_provider_summary::ProviderSummary {
        self.loader_facts()
            .module_provider_summary(module_id)
            .unwrap_or_else(|| {
                db.invalid_input(
                    &ExtensionProviderSummaryQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn path_for_module(&self, module_id: ModuleId) -> SourcePath {
        self.loaded_module(module_id)
            .unwrap_or_else(|| panic!("Nia ICE: missing loaded module {module_id:?}"))
            .path
            .clone()
    }

    fn module_graph_entry_key(&self) -> StableModuleKey {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        inputs
            .graph
            .stable_key(inputs.graph.entry())
            .cloned()
            .expect("compiler entry must have a stable module key")
    }

    fn module_graph_path(&self, module_id: ModuleId) -> Option<nia_imports::ModulePath> {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .graph
            .get(module_id)
            .map(|module| module.module_path.clone())
    }

    fn module_graph_parent_key(&self, module_id: ModuleId) -> Option<StableModuleKey> {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        let parent = inputs.graph.get(module_id)?.parent?;
        inputs.graph.stable_key(parent).cloned()
    }

    fn module_graph_child_key(
        &self,
        module_id: ModuleId,
        name: &SymbolId,
    ) -> Option<(StableModuleKey, nia_ids::Visibility)> {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        let module = inputs.graph.get(module_id)?;
        let target = module.children.get(name).copied()?;
        let declaration = module
            .declarations
            .iter()
            .find(|declaration| declaration.name == *name && declaration.target == target)?;
        Some((
            inputs.graph.stable_key(target)?.clone(),
            declaration.visibility,
        ))
    }

    fn module_package_root_key(&self, package: &SymbolId) -> Option<StableModuleKey> {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        let root = inputs.graph.package_root(package)?;
        inputs.graph.stable_key(root).cloned()
    }

    fn module_id_for_stable_key(&self, stable_key: &StableModuleKey) -> Option<ModuleId> {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .graph
            .module_id_for_stable_key(stable_key)
    }

    fn symbols(&self) -> nia_symbol_table::SymbolTable {
        self.loader_facts().symbols()
    }

    fn provider_fact_worklist(&self) -> ProviderFactWorklist {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        ProviderFactWorklist {
            revision: inputs.provider_fact_revision,
            changes: Arc::clone(&inputs.provider_worklist),
        }
    }

    fn body_activation_worklist(&self) -> BodyActivationWorklist {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        BodyActivationWorklist {
            modules: Arc::clone(&inputs.body_activation_worklist),
        }
    }

    fn executable_fact_epoch(&self) -> ExecutableFactEpoch {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .executable_fact_epoch
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

fn index_input_modules(modules: &[CompilerInputModule]) -> HashMap<ModuleId, usize> {
    let mut modules_by_id = HashMap::new();
    for (index, module) in modules.iter().enumerate() {
        if let Some(existing) = modules_by_id.insert(module.id, index) {
            panic!(
                "Nia ICE: duplicate loaded module id {:?} at indexes {existing} and {index}",
                module.id
            );
        }
    }
    modules_by_id
}

fn index_input_module_identities(
    modules: &[CompilerInputModule],
) -> HashMap<SourceIdentity, usize> {
    let mut modules_by_source_identity = HashMap::new();
    for (index, module) in modules.iter().enumerate() {
        if let Some(existing) =
            modules_by_source_identity.insert(module.source_identity.clone(), index)
        {
            panic!(
                "Nia ICE: duplicate source identity `{}` for loaded modules {:?} and {:?}",
                module.source_identity.normalized_path(),
                modules[existing].id,
                module.id
            );
        }
    }
    modules_by_source_identity
}

#[derive(Debug, Default)]
struct CompilerInputDiff {
    graph_changed: bool,
    graph_entry_changed: bool,
    changed_graph_paths: HashSet<ModuleId>,
    changed_graph_parents: HashSet<ModuleId>,
    changed_graph_children: HashSet<(ModuleId, SymbolId)>,
    changed_package_roots: HashSet<SymbolId>,
    executable_roots_changed: bool,
    body_activated_modules: HashSet<ModuleId>,
    provider_facts_reset: bool,
    executable_fact_inputs_changed: bool,
    loaded_modules_changed: bool,
    loaded_diagnostics_changed: bool,
    target_changed: bool,
    runtime_changed: bool,
    optimization_changed: bool,
    changed_modules: Vec<ChangedModuleInput>,
}

impl CompilerInputDiff {
    #[cfg(test)]
    fn between(old: &CompilerInputs, new: &CompilerInputs) -> Self {
        Self::try_between(old, new).unwrap_or_else(|error| panic!("Nia ICE: {error}"))
    }

    fn try_between(
        old: &CompilerInputs,
        new: &CompilerInputs,
    ) -> Result<Self, ProviderFactInputError> {
        let provider_facts_reset = validate_provider_fact_update(old, new)?;
        let changed_modules = changed_loaded_modules(old, new);
        let executable_roots_changed = old.entry_module != new.entry_module
            || old.runtime_root_modules != new.runtime_root_modules;
        let executable_fact_inputs_changed = executable_fact_inputs_changed(old, new);
        let body_activated_modules = new
            .graph
            .modules()
            .filter(|node| {
                node.process_used_paths
                    && old
                        .graph
                        .get(node.id)
                        .is_some_and(|old| !old.process_used_paths)
            })
            .map(|node| node.id)
            .collect::<HashSet<_>>();
        Ok(Self {
            graph_changed: old.graph != new.graph,
            graph_entry_changed: old.graph.entry() != new.graph.entry(),
            changed_graph_paths: changed_graph_paths(old, new),
            changed_graph_parents: changed_graph_parents(old, new),
            changed_graph_children: changed_graph_children(old, new),
            changed_package_roots: changed_package_roots(old, new),
            executable_roots_changed,
            body_activated_modules,
            provider_facts_reset,
            executable_fact_inputs_changed,
            loaded_modules_changed: loaded_module_ids(old) != loaded_module_ids(new)
                || loaded_module_identity_assignments(old)
                    != loaded_module_identity_assignments(new),
            loaded_diagnostics_changed: old.diagnostics != new.diagnostics,
            target_changed: old.target != new.target,
            runtime_changed: old.runtime != new.runtime,
            optimization_changed: old.optimization != new.optimization,
            changed_modules,
        })
    }

    fn resets_executable_facts(&self) -> bool {
        self.executable_fact_inputs_changed
            || self.provider_facts_reset
            || self.target_changed
            || self.runtime_changed
            || self.executable_roots_changed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderFactInputError {
    ChangesAtUnchangedRevision,
    ChangesFromReplacementOwner,
    StaleRevision,
}

impl std::fmt::Display for ProviderFactInputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ChangesAtUnchangedRevision => {
                formatter.write_str("provider changes require an advanced provider fact revision")
            }
            Self::ChangesFromReplacementOwner => {
                formatter.write_str("provider fact owner replacement cannot carry provider changes")
            }
            Self::StaleRevision => {
                formatter.write_str("provider fact revision cannot move backwards")
            }
        }
    }
}

fn validate_provider_fact_update(
    old: &CompilerInputs,
    new: &CompilerInputs,
) -> Result<bool, ProviderFactInputError> {
    use crate::ProviderFactRevisionTransition::{Advanced, Replaced, Stale, Unchanged};

    match (
        new.provider_fact_revision
            .transition_from(old.provider_fact_revision),
        new.provider_worklist.is_empty(),
    ) {
        (Unchanged, true) | (Advanced, false) => Ok(false),
        (Advanced | Replaced, true) => Ok(true),
        (Unchanged, false) => Err(ProviderFactInputError::ChangesAtUnchangedRevision),
        (Replaced, false) => Err(ProviderFactInputError::ChangesFromReplacementOwner),
        (Stale, _) => Err(ProviderFactInputError::StaleRevision),
    }
}

fn executable_fact_inputs_changed(old: &CompilerInputs, new: &CompilerInputs) -> bool {
    old.modules.iter().any(|old_module| {
        let Some(new_module) = new.loaded_module_by_source_identity(&old_module.source_identity)
        else {
            return true;
        };
        old_module.id != new_module.id
            || ChangedModuleInput::between_source_identity(Some(old_module), Some(new_module))
                .is_some()
    })
}

fn changed_graph_paths(old: &CompilerInputs, new: &CompilerInputs) -> HashSet<ModuleId> {
    old.graph
        .modules()
        .map(|module| module.id)
        .chain(new.graph.modules().map(|module| module.id))
        .filter(|module_id| {
            old.graph.get(*module_id).map(|module| &module.module_path)
                != new.graph.get(*module_id).map(|module| &module.module_path)
        })
        .collect()
}

fn changed_graph_parents(old: &CompilerInputs, new: &CompilerInputs) -> HashSet<ModuleId> {
    old.graph
        .modules()
        .map(|module| module.id)
        .chain(new.graph.modules().map(|module| module.id))
        .filter(|module_id| {
            old.graph.get(*module_id).and_then(|module| module.parent)
                != new.graph.get(*module_id).and_then(|module| module.parent)
        })
        .collect()
}

fn changed_graph_children(
    old: &CompilerInputs,
    new: &CompilerInputs,
) -> HashSet<(ModuleId, SymbolId)> {
    let module_ids = old
        .graph
        .modules()
        .map(|module| module.id)
        .chain(new.graph.modules().map(|module| module.id))
        .collect::<HashSet<_>>();
    let mut changed = HashSet::new();
    for module_id in module_ids {
        let names = old
            .graph
            .get(module_id)
            .into_iter()
            .flat_map(|module| module.children.keys().copied())
            .chain(
                new.graph
                    .get(module_id)
                    .into_iter()
                    .flat_map(|module| module.children.keys().copied()),
            )
            .collect::<HashSet<_>>();
        for name in names {
            let old_child = old
                .graph
                .get(module_id)
                .and_then(|module| graph_child_declaration(module, name));
            let new_child = new
                .graph
                .get(module_id)
                .and_then(|module| graph_child_declaration(module, name));
            if old_child != new_child {
                changed.insert((module_id, name));
            }
        }
    }
    changed
}

fn graph_child_declaration(
    module: &ModuleNode,
    name: SymbolId,
) -> Option<(ModuleId, nia_ids::Visibility)> {
    let target = module.children.get(&name).copied()?;
    let declaration = module
        .declarations
        .iter()
        .find(|declaration| declaration.name == name && declaration.target == target)?;
    Some((target, declaration.visibility))
}

fn changed_package_roots(old: &CompilerInputs, new: &CompilerInputs) -> HashSet<SymbolId> {
    old.graph
        .modules()
        .map(|module| module.module_path.package)
        .chain(new.graph.modules().map(|module| module.module_path.package))
        .filter(|package| old.graph.package_root(package) != new.graph.package_root(package))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChangedModuleInput {
    ids: Vec<ModuleId>,
    path: bool,
    source_identity: bool,
    source_version: bool,
    origins: bool,
    parse_errors: bool,
    item_tree: bool,
    declaration_item_tree: bool,
    full_item_tree: bool,
    active_item_tree: bool,
    declaration_active_item_tree: bool,
    signature_function_items: bool,
    signature_extension_function_items: bool,
    signature_value_items: bool,
    signature_type_items: bool,
    provider_summary: bool,
    signature_trait_items: bool,
    signature_const_items: bool,
    full_active_item_tree: bool,
}

impl ChangedModuleInput {
    fn between_source_identity(
        old: Option<&CompilerInputModule>,
        new: Option<&CompilerInputModule>,
    ) -> Option<Self> {
        let ids = changed_module_ids(old, new);
        if ids.is_empty() {
            return None;
        }

        let changed = match (old, new) {
            (Some(old), Some(new)) if old.id == new.id => {
                let source_revision_changed = old.source_version != new.source_version;
                Self {
                    ids,
                    path: old.path != new.path,
                    source_identity: old.source_identity != new.source_identity,
                    source_version: source_revision_changed,
                    origins: source_revision_changed || old.origins != new.origins,
                    parse_errors: old.parse_errors != new.parse_errors,
                    item_tree: source_revision_changed
                        || !old.item_tree.definition_eq(&new.item_tree),
                    declaration_item_tree: source_revision_changed
                        || !old.item_tree.declaration_eq(&new.item_tree),
                    full_item_tree: source_revision_changed || old.item_tree != new.item_tree,
                    active_item_tree: source_revision_changed
                        || !old.active_item_tree.definition_eq(&new.active_item_tree),
                    declaration_active_item_tree: source_revision_changed
                        || !old.active_item_tree.declaration_eq(&new.active_item_tree),
                    signature_function_items: source_revision_changed
                        || !old
                            .active_item_tree
                            .signature_items(nia_item_tree::SignatureItemSet::Functions)
                            .declaration_eq(
                                &new.active_item_tree
                                    .signature_items(nia_item_tree::SignatureItemSet::Functions),
                            ),
                    signature_extension_function_items: source_revision_changed
                        || !old
                            .active_item_tree
                            .signature_items(nia_item_tree::SignatureItemSet::ExtensionFunctions)
                            .declaration_eq(&new.active_item_tree.signature_items(
                                nia_item_tree::SignatureItemSet::ExtensionFunctions,
                            )),
                    signature_value_items: source_revision_changed
                        || !old
                            .active_item_tree
                            .signature_items(nia_item_tree::SignatureItemSet::Values)
                            .declaration_eq(
                                &new.active_item_tree
                                    .signature_items(nia_item_tree::SignatureItemSet::Values),
                            ),
                    signature_type_items: source_revision_changed
                        || !old
                            .active_item_tree
                            .signature_items(nia_item_tree::SignatureItemSet::Types)
                            .declaration_eq(
                                &new.active_item_tree
                                    .signature_items(nia_item_tree::SignatureItemSet::Types),
                            ),
                    provider_summary: old.provider_summary != new.provider_summary,
                    signature_trait_items: source_revision_changed
                        || !old
                            .active_item_tree
                            .signature_items(nia_item_tree::SignatureItemSet::Traits)
                            .declaration_eq(
                                &new.active_item_tree
                                    .signature_items(nia_item_tree::SignatureItemSet::Traits),
                            ),
                    signature_const_items: source_revision_changed
                        || old.active_item_tree.const_signature_items()
                            != new.active_item_tree.const_signature_items(),
                    full_active_item_tree: source_revision_changed
                        || old.active_item_tree != new.active_item_tree,
                }
            }
            (Some(_), Some(_)) => Self::all_inputs_changed(ids),
            (Some(_), None) | (None, Some(_)) => Self {
                ids,
                path: true,
                source_identity: true,
                source_version: true,
                origins: true,
                parse_errors: true,
                item_tree: true,
                declaration_item_tree: true,
                full_item_tree: true,
                active_item_tree: true,
                declaration_active_item_tree: true,
                signature_function_items: true,
                signature_extension_function_items: true,
                signature_value_items: true,
                signature_type_items: true,
                provider_summary: true,
                signature_trait_items: true,
                signature_const_items: true,
                full_active_item_tree: true,
            },
            (None, None) => return None,
        };
        if changed.path
            || changed.source_identity
            || changed.source_version
            || changed.origins
            || changed.parse_errors
            || changed.item_tree
            || changed.declaration_item_tree
            || changed.full_item_tree
            || changed.active_item_tree
            || changed.declaration_active_item_tree
            || changed.signature_function_items
            || changed.signature_extension_function_items
            || changed.signature_value_items
            || changed.signature_type_items
            || changed.provider_summary
            || changed.signature_trait_items
            || changed.signature_const_items
            || changed.full_active_item_tree
        {
            Some(changed)
        } else {
            None
        }
    }

    fn all_inputs_changed(ids: Vec<ModuleId>) -> Self {
        Self {
            ids,
            path: true,
            source_identity: true,
            source_version: true,
            origins: true,
            parse_errors: true,
            item_tree: true,
            declaration_item_tree: true,
            full_item_tree: true,
            active_item_tree: true,
            declaration_active_item_tree: true,
            signature_function_items: true,
            signature_extension_function_items: true,
            signature_value_items: true,
            signature_type_items: true,
            provider_summary: true,
            signature_trait_items: true,
            signature_const_items: true,
            full_active_item_tree: true,
        }
    }
}

fn changed_loaded_modules(old: &CompilerInputs, new: &CompilerInputs) -> Vec<ChangedModuleInput> {
    let source_identities = old
        .modules
        .iter()
        .map(|module| module.source_identity.clone())
        .chain(
            new.modules
                .iter()
                .map(|module| module.source_identity.clone()),
        )
        .collect::<HashSet<_>>();
    let mut changed = source_identities
        .into_iter()
        .filter_map(|source_identity| {
            ChangedModuleInput::between_source_identity(
                old.loaded_module_by_source_identity(&source_identity),
                new.loaded_module_by_source_identity(&source_identity),
            )
        })
        .collect::<Vec<_>>();
    changed.sort_by_key(|module| module.ids.first().map_or(u32::MAX, |id| id.local_index()));
    changed
}

fn changed_module_ids(
    old: Option<&CompilerInputModule>,
    new: Option<&CompilerInputModule>,
) -> Vec<ModuleId> {
    let mut ids = Vec::new();
    if let Some(module) = old {
        ids.push(module.id);
    }
    if let Some(module) = new {
        ids.push(module.id);
    }
    ids.sort();
    ids.dedup();
    ids
}

fn loaded_module_ids(inputs: &CompilerInputs) -> Vec<ModuleId> {
    inputs.modules.iter().map(|module| module.id).collect()
}

fn loaded_module_identity_assignments(inputs: &CompilerInputs) -> Vec<(ModuleId, SourceIdentity)> {
    let mut assignments = inputs
        .modules
        .iter()
        .map(|module| (module.id, module.source_identity.clone()))
        .collect::<Vec<_>>();
    assignments.sort_by_key(|(id, _)| *id);
    assignments
}

impl CompilerInputs {
    fn loaded_module(&self, module_id: ModuleId) -> Option<&CompilerInputModule> {
        self.modules_by_id
            .get(&module_id)
            .and_then(|index| self.modules.get(*index))
    }

    fn loaded_module_by_source_identity(
        &self,
        source_identity: &SourceIdentity,
    ) -> Option<&CompilerInputModule> {
        self.modules_by_source_identity
            .get(source_identity)
            .and_then(|index| self.modules.get(*index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RuntimeModel;
    use nia_sema_ir::SemanticValueUse;
    use nia_source::{SourceId, SourceRevision};
    use nia_symbol::{SymbolId, stable_hash};

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
        let inputs = Arc::new(RwLock::new(CompilerInputs::new(CompileRequest::new(
            loaded,
        ))));
        let node_store = inputs
            .read()
            .expect("compiler input lock poisoned")
            .modules
            .first()
            .map(|module| module.origins.node_store().clone())
            .unwrap_or_default();
        QueryDb::new_registered(
            CompilerContext {
                inputs,
                loader_facts: Arc::new(RwLock::new(loader_facts)),
                providers: CompilerQueryProviders::default(),
                executable_checked_modules: Arc::new(RwLock::new(
                    ExecutableCheckedModuleStore::default(),
                )),
                executable_fact_session: Arc::new(std::sync::Mutex::new(
                    ExecutableFactSession::default(),
                )),
                type_store: Arc::new(nia_ty::TypeStore::new()),
                node_store,
            },
            compiler_query_registry(),
        )
    }

    fn module_id_for_source_identity(
        db: &QueryDb<CompilerContext>,
        identity: &SourceIdentity,
    ) -> Option<ModuleId> {
        let inputs = db
            .context()
            .inputs
            .read()
            .expect("compiler input lock poisoned");
        inputs
            .loaded_module_by_source_identity(identity)
            .map(|module| module.id)
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

        assert_eq!(descriptors.len(), 120);
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
            descriptor.context_type == std::any::type_name::<CompilerContext>()
                && descriptor.provider == nia_query::QueryProviderPolicy::KeyExecute
                && descriptor.storage == nia_query::QueryStoragePolicy::CacheOwnedArc
        }));
        for descriptor in descriptors {
            let expected = match descriptor.name {
                "body_activation_worklist"
                | "executable_fact_epoch"
                | "extension_provider_module_ids"
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
                | "declaration_active_module_item_tree_input"
                | "declaration_module_item_tree_input"
                | "full_active_module_item_tree_input"
                | "full_module_item_tree_input"
                | "module_public_surface"
                | "module_item_tree_input"
                | "module_origins"
                | "module_parse_errors"
                | "module_using_scope"
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
            resolve_stable_module_sequence(&db, &db.get(ParseOkModuleIdsQuery)).as_slice(),
            &[entry_id, facade_id]
        );
        assert_eq!(
            resolve_stable_module_sequence(&db, &db.get(SemanticModuleIdsQuery)).as_slice(),
            &[entry_id]
        );

        assert_eq!(db.get(CheckedModuleIdsQuery).as_slice(), &[entry_id]);
    }

    #[test]
    fn compiler_inputs_index_modules_by_source_identity() {
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
    #[should_panic(expected = "Nia ICE: duplicate loaded module id")]
    fn compiler_inputs_reject_duplicate_module_ids() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let module_id = fixture.entry_id();
        let mut program = fixture.program();
        program.modules.push(loaded_module(
            module_id,
            "other.nia",
            "pub fn value() i32 { 1 }",
        ));
        let _ = CompilerInputs::new(CompileRequest::new(program));
    }

    #[test]
    #[should_panic(expected = "Nia ICE: duplicate source identity")]
    fn compiler_inputs_reject_duplicate_source_identities() {
        let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let entry_id = fixture.entry_id();
        fixture.add_child(
            entry_id,
            "duplicate",
            "main.nia",
            "pub fn value() i32 { 1 }",
        );
        let _ = CompilerInputs::new(CompileRequest::new(fixture.program()));
    }

    #[test]
    #[should_panic(expected = "Nia ICE: loaded module")]
    fn compiler_inputs_reject_path_identity_mismatch() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let mut program = fixture.program();
        program.modules[0].source_identity = SourcePath::new("other.nia").identity();
        let _ = CompilerInputs::new(CompileRequest::new(program));
    }

    #[test]
    fn loaded_module_reorder_invalidates_list_without_field_changes() {
        let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let entry_id = fixture.entry_id();
        let package_id =
            fixture.add_child(entry_id, "pkg", "pkg/root.nia", "pub fn value() i32 { 1 }");
        let old = CompilerInputs::new(CompileRequest::new(fixture.program()));
        let database = fixture.database();
        let first = database.db.get(LoadedModulesQuery);
        let mut reordered = fixture.program();
        reordered.modules.reverse();
        let new = CompilerInputs::new(CompileRequest::new(reordered.clone()));

        let diff = CompilerInputDiff::between(&old, &new);

        assert!(diff.loaded_modules_changed);
        assert_eq!(diff.changed_modules, Vec::new());
        database.update(CompileRequest::new(reordered));
        let latest = database.db.get(LoadedModulesQuery);
        assert!(!Arc::ptr_eq(&first, &latest));
        assert_eq!(
            resolve_stable_module_sequence(&database.db, &latest),
            vec![package_id, entry_id]
        );
    }

    #[test]
    fn additive_module_growth_preserves_existing_executable_fact_inputs() {
        let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let old = CompilerInputs::new(CompileRequest::new(fixture.program()));
        let entry_id = fixture.entry_id();
        fixture.add_child(
            entry_id,
            "provider",
            "main/provider.nia",
            "pub fn value() i32 { 1 }",
        );
        let new = CompilerInputs::new(CompileRequest::new(fixture.program()));

        let diff = CompilerInputDiff::between(&old, &new);

        assert!(diff.loaded_modules_changed);
        assert!(!diff.executable_fact_inputs_changed);
    }

    #[test]
    fn stable_graph_entry_remaps_after_module_graph_owner_replacement() {
        let old_fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let new_fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let old_entry = old_fixture.entry_id();
        let new_entry = new_fixture.entry_id();
        assert_ne!(old_entry, new_entry);
        let database = old_fixture.database();
        let first = database.db.get(ModuleGraphEntryQuery);
        let first_loaded = database.db.get(LoadedModulesQuery);

        database.update(CompileRequest::new(new_fixture.program()));

        let latest = database.db.get(ModuleGraphEntryQuery);
        let latest_loaded = database.db.get(LoadedModulesQuery);
        assert!(Arc::ptr_eq(&first, &latest));
        assert!(Arc::ptr_eq(&first_loaded, &latest_loaded));
        assert_eq!(
            resolve_stable_module_sequence(&database.db, &latest_loaded),
            vec![new_entry]
        );
        assert_eq!(
            QueryModuleGraphLookup::new(&database.db).entry_module(),
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
        let first_child = database.db.get(ModuleGraphChildQuery(entry, child_name));
        let first_root = database.db.get(ModulePackageRootQuery(package));
        let first_public = database.db.get(PublicSurfaceModuleQuery(entry, child_name));
        let first_using = database.db.get(UsingScopeModuleQuery(entry, child_name));

        database.update(CompileRequest::new(new_fixture.program()));

        let latest_child = database.db.get(ModuleGraphChildQuery(entry, child_name));
        let latest_root = database.db.get(ModulePackageRootQuery(package));
        let latest_public = database.db.get(PublicSurfaceModuleQuery(entry, child_name));
        let latest_using = database.db.get(UsingScopeModuleQuery(entry, child_name));
        assert!(Arc::ptr_eq(&first_child, &latest_child));
        assert!(Arc::ptr_eq(&first_root, &latest_root));
        assert!(!Arc::ptr_eq(&first_public, &latest_public));
        assert!(!Arc::ptr_eq(&first_using, &latest_using));
        assert_eq!(first_public.as_ref(), latest_public.as_ref());
        assert_eq!(first_using.as_ref(), latest_using.as_ref());
        let lookup = QueryModuleGraphLookup::new(&database.db);
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
        let old_module_id = old_fixture.entry_id();
        let old_program = old_fixture.program();

        let mut new_fixture = LoadedProgramFixture::new("bootstrap.nia", "");
        let new_module_id = new_fixture
            .graph
            .intern_package_root(&sym("replacement"), SourcePath::new("main.nia"));
        new_fixture.graph.mark_process_used_paths(new_module_id);
        new_fixture.modules = vec![loaded_module(new_module_id, "main.nia", source)];
        let new_program = new_fixture.program();

        let old = CompilerInputs::new(CompileRequest::new(old_program.clone()));
        let new = CompilerInputs::new(CompileRequest::new(new_program.clone()));
        let diff = CompilerInputDiff::between(&old, &new);

        assert!(diff.loaded_modules_changed);
        assert_eq!(diff.changed_modules.len(), 1);
        assert_eq!(
            diff.changed_modules[0].ids,
            vec![old_module_id, new_module_id]
        );

        let database = CompilerDatabase::new(CompileRequest::new(old_program));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
        let first_loaded = database.db.get(LoadedModulesQuery);

        let invalidation = database.update(CompileRequest::new(new_program));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.description.as_str())
            .collect::<Vec<_>>();

        let old_module_path = format!("module_path({old_module_id:?})");
        let old_checked_module = format!("checked_module::CheckedModuleQuery({old_module_id:?})");
        assert!(
            invalidated.contains(&old_module_path.as_str()),
            "{invalidated:?}"
        );
        assert!(
            invalidated.contains(&old_checked_module.as_str()),
            "{invalidated:?}"
        );
        assert!(
            !invalidated.contains(&"loaded_modules::LoadedModulesQuery"),
            "{invalidated:?}"
        );
        let latest_loaded = database.db.get(LoadedModulesQuery);
        assert!(Arc::ptr_eq(&first_loaded, &latest_loaded));
        assert_eq!(
            resolve_stable_module_sequence(&database.db, &latest_loaded),
            vec![new_module_id]
        );

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        assert_eq!(second.modules[0].id, new_module_id);
    }

    #[test]
    fn same_module_id_with_new_source_identity_is_replacement() {
        let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let module_id = fixture.entry_id();
        let old = CompilerInputs::new(CompileRequest::new(fixture.program()));
        fixture.update_module_path(module_id, "other.nia");
        let new = CompilerInputs::new(CompileRequest::new(fixture.program()));

        let diff = CompilerInputDiff::between(&old, &new);

        assert!(diff.loaded_modules_changed);
        assert_eq!(diff.changed_modules.len(), 2);
        assert!(diff.changed_modules.iter().all(|module| {
            module.ids == vec![module_id]
                && module.path
                && module.source_identity
                && module.source_version
                && module.item_tree
                && module.full_item_tree
        }));
    }

    #[test]
    fn compiler_database_update_invalidates_changed_module_field_inputs() {
        let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let module_id = fixture.entry_id();
        let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
        let first_source_version = database.db.get(ModuleSourceVersionQuery(module_id));

        fixture.update_module_source(module_id, "fn main() i32 { true }", SourceRevision(1));
        let invalidation = database.update(CompileRequest::new(fixture.program()));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(
            invalidated.contains(&"module_source_version"),
            "{invalidated:?}"
        );
        assert!(
            invalidated.contains(&"module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(
            invalidated.contains(&"full_module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"checked_program"), "{invalidated:?}");
        let latest_source_version = database.db.get(ModuleSourceVersionQuery(module_id));
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
        let first = database.db.get(ModuleSourceVersionQuery(module_id));
        let replacement = SourceVersion {
            id: SourceId(first.id.0 + 1),
            revision: first.revision,
        };
        fixture.modules[0] =
            loaded_module_with_source_version(module_id, "main.nia", source, replacement);

        let invalidation = database.update(CompileRequest::new(fixture.program()));
        let latest = database.db.get(ModuleSourceVersionQuery(module_id));

        assert!(
            invalidation
                .invalidated
                .iter()
                .any(|frame| frame.name == "module_source_version")
        );
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
    fn provider_graph_growth_keeps_executable_roots_cached() {
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
            database.db.get(ExecutableRootModulesQuery).as_ref(),
            &(entry_id, Vec::new())
        );
        let _ = database.db.get(TypeResolutionQuery(entry_id));

        let mut grown = loaded;
        let mut graph = (*grown.graph).clone();
        assert!(graph.mark_semantic_selected(provider_id));
        grown.graph = graph.into();
        let invalidation = database.update(CompileRequest::new(grown));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(invalidated.contains(&"module_graph"), "{invalidated:?}");
        assert!(!invalidated.contains(&"type_resolution"), "{invalidated:?}");
        assert!(
            !invalidated.contains(&"executable_root_modules"),
            "{invalidated:?}"
        );
        assert_eq!(
            database.db.get(ExecutableRootModulesQuery).as_ref(),
            &(entry_id, Vec::new())
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
        let database = CompilerDatabase::new(
            CompileRequest::new(fixture.program()).with_provider_fact_revision(revision),
        );
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
        database.update(
            CompileRequest::new(fixture.program())
                .with_provider_fact_revision(revision.next())
                .with_provider_changes(provider_changes),
        );

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
        let worklist = database.db.get(ProviderFactWorklistQuery);
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
    fn compile_request_deduplicates_provider_changes() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let demand = crate::ProviderDemand {
            source_path: SourcePath::new("main.nia"),
            request: crate::ProviderRequest::Method {
                target_type_name: None,
                method_name: SymbolId::default(),
            },
        };
        let request =
            CompileRequest::new(fixture.program()).with_provider_changes([demand.clone(), demand]);
        assert_eq!(request.provider_changes.len(), 1);
        let database = CompilerDatabase::new(request);

        let inputs = database
            .inputs
            .read()
            .expect("compiler input lock poisoned");
        assert_eq!(inputs.provider_worklist.len(), 1);
    }

    #[test]
    fn compiler_inputs_preserve_provider_fact_revision() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let mut program = fixture.program();
        let revision = crate::ProviderFactRevision::new_store().next();
        program.provider_fact_revision = revision;
        let database = CompilerDatabase::new(CompileRequest::new(program));

        assert_eq!(database.provider_fact_revision(), revision);
    }

    #[test]
    fn executable_products_depend_on_incremental_worklists() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let revision = crate::ProviderFactRevision::new_store();
        let database = CompilerDatabase::new(
            CompileRequest::new(fixture.program()).with_provider_fact_revision(revision),
        );
        assert_eq!(std::mem::size_of::<ProviderFactWorklistQuery>(), 0);
        assert_eq!(std::mem::size_of::<BodyActivationWorklistQuery>(), 0);
        assert_eq!(std::mem::size_of::<ExecutableFactEpochQuery>(), 0);
        assert_eq!(std::mem::size_of::<ExecutableFactEpoch>(), 8);

        let _ = database.executable_provider_demands();
        let _ = database.db.get(ExecutableCheckedModuleSetQuery);
        assert_eq!(database.provider_fact_revision(), revision);

        let dependencies = &database.query_trace().dependencies;
        for product in [
            "executable_provider_demands",
            "executable_checked_module_set",
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
            dependency.from.name == "provider_fact_revision"
                && dependency.to.name == "provider_fact_worklist"
        }));
    }

    #[test]
    fn compiler_input_fact_fingerprints_are_deterministic_and_order_independent() {
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
        let first_provider = ProviderFactWorklist {
            revision,
            changes: Arc::new(HashSet::from([method.clone(), trait_impl.clone()])),
        };
        let mut reversed_changes = HashSet::new();
        reversed_changes.insert(trait_impl);
        reversed_changes.insert(method);
        let second_provider = ProviderFactWorklist {
            revision,
            changes: Arc::new(reversed_changes),
        };
        assert_eq!(
            provider_fact_worklist_fingerprint(&first_provider),
            provider_fact_worklist_fingerprint(&second_provider)
        );

        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let entry = fixture.entry_id();
        let stable_key = fixture
            .graph
            .stable_key(entry)
            .expect("entry stable key")
            .clone();
        let first_body = BodyActivationWorklist {
            modules: Arc::new(HashMap::from([(stable_key.clone(), entry)])),
        };
        let mut second_modules = HashMap::new();
        second_modules.insert(stable_key, entry);
        let second_body = BodyActivationWorklist {
            modules: Arc::new(second_modules),
        };
        assert_eq!(
            body_activation_worklist_fingerprint(&first_body),
            body_activation_worklist_fingerprint(&second_body)
        );
        assert_eq!(
            executable_fact_epoch_fingerprint(ExecutableFactEpoch::default()),
            executable_fact_epoch_fingerprint(ExecutableFactEpoch::default())
        );
        assert_ne!(
            executable_fact_epoch_fingerprint(ExecutableFactEpoch::default()),
            executable_fact_epoch_fingerprint(ExecutableFactEpoch::default().next())
        );
    }

    #[test]
    fn executable_fact_epoch_defers_full_reset_to_query_boundary() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let database = fixture.database();
        let _ = database.db.get(ExecutableCheckedModuleSetQuery);
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
            assert_eq!(session.epoch, Some(ExecutableFactEpoch::default()));
            session.applied_provider_changes.insert(sentinel.clone());
        }

        let mut reset = fixture.program();
        reset.runtime = RuntimeModel::FreestandingExecutable;
        let invalidation = database.update(CompileRequest::new(reset));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();
        for name in [
            "executable_fact_epoch",
            "executable_provider_demands",
            "executable_checked_module_set",
        ] {
            assert!(invalidated.contains(&name), "{invalidated:?}");
        }
        {
            let session = database
                .db
                .context()
                .executable_fact_session
                .lock()
                .expect("executable fact session lock poisoned");
            assert_eq!(session.epoch, Some(ExecutableFactEpoch::default()));
            assert!(session.applied_provider_changes.contains(&sentinel));
        }

        let _ = database.executable_provider_demands();
        let session = database
            .db
            .context()
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned");
        assert_eq!(session.epoch, Some(ExecutableFactEpoch::default().next()));
        assert!(!session.applied_provider_changes.contains(&sentinel));
    }

    #[test]
    fn provider_revision_update_invalidates_executable_products() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let revision = crate::ProviderFactRevision::new_store();
        let database = CompilerDatabase::new(
            CompileRequest::new(fixture.program()).with_provider_fact_revision(revision),
        );
        let _ = database.executable_provider_demands();
        let first_set = database.db.get(ExecutableCheckedModuleSetQuery);
        assert_eq!(database.provider_fact_revision(), revision);

        let invalidation = database.update(
            CompileRequest::new(fixture.program()).with_provider_fact_revision(revision.next()),
        );
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        for name in [
            "provider_fact_revision",
            "provider_fact_worklist",
            "executable_provider_demands",
            "executable_checked_module_set",
        ] {
            assert!(invalidated.contains(&name), "{invalidated:?}");
        }
        assert!(
            !invalidated.contains(&"body_activation_worklist"),
            "{invalidated:?}"
        );
        assert_eq!(database.provider_fact_revision(), revision.next());
        let revision_query = database
            .query_trace()
            .queries
            .into_iter()
            .find(|query| query.frame.name == "provider_fact_revision")
            .expect("provider fact revision query trace");
        assert_eq!(revision_query.stats.validations, 1);
        assert_eq!(revision_query.stats.green_validations, 0);
        let second_set = database.db.get(ExecutableCheckedModuleSetQuery);
        assert_ne!(first_set.id, second_set.id);
        assert!(
            !database
                .db
                .context()
                .executable_checked_modules(&second_set)
                .is_empty()
        );
    }

    #[test]
    fn provider_worklist_accumulates_until_consumed() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let revision = crate::ProviderFactRevision::new_store();
        let database = CompilerDatabase::new(
            CompileRequest::new(fixture.program()).with_provider_fact_revision(revision),
        );
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

        database.update(
            CompileRequest::new(fixture.program())
                .with_provider_fact_revision(first_revision)
                .with_provider_changes([first_demand.clone()]),
        );
        database.update(
            CompileRequest::new(fixture.program())
                .with_provider_fact_revision(second_revision)
                .with_provider_changes([second_demand.clone()]),
        );
        database.update(
            CompileRequest::new(fixture.program()).with_provider_fact_revision(second_revision),
        );

        let worklist = database.db.get(ProviderFactWorklistQuery);
        let expected_changes = HashSet::from([first_demand, second_demand]);
        assert_eq!(worklist.revision, second_revision);
        assert_eq!(worklist.changes.as_ref(), &expected_changes);

        let mut session = ExecutableFactSession::default();
        session.apply_provider_fact_worklist(&worklist, &database.db.context().type_store);
        assert_eq!(
            session.applied_provider_fact_revision,
            Some(second_revision)
        );
        assert_eq!(session.applied_provider_changes, expected_changes);

        let reset_revision = second_revision.next();
        database.update(
            CompileRequest::new(fixture.program()).with_provider_fact_revision(reset_revision),
        );
        let reset = database.db.get(ProviderFactWorklistQuery);
        assert_eq!(reset.revision, reset_revision);
        assert!(reset.changes.is_empty());
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
        let first_invalidation = database.update(CompileRequest::new(fixture.program()));
        assert!(
            first_invalidation
                .invalidated
                .iter()
                .any(|frame| frame.name == "body_activation_worklist")
        );
        assert!(
            first_invalidation
                .invalidated
                .iter()
                .any(|frame| frame.name == "executable_provider_demands")
        );

        assert!(fixture.graph.mark_process_used_paths(second_module));
        database.update(CompileRequest::new(fixture.program()));

        let worklist = database.db.get(BodyActivationWorklistQuery);
        let expected = HashMap::from([
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
        let database = CompilerDatabase::new(
            CompileRequest::new(fixture.program()).with_provider_fact_revision(revision),
        );
        let first_set = database.db.get(ExecutableCheckedModuleSetQuery);
        let _ = database.executable_provider_demands();
        let before_update = database.query_trace();

        let invalidation = database.update(
            CompileRequest::new(fixture.program())
                .with_provider_fact_revision(revision)
                .with_timings(crate::TimingMode::Summary),
        );

        assert!(
            invalidation.invalidated.is_empty(),
            "{:?}",
            invalidation.invalidated
        );
        let second_set = database.db.get(ExecutableCheckedModuleSetQuery);
        let _ = database.executable_provider_demands();
        assert_eq!(first_set.id, second_set.id);
        assert!(
            !database
                .db
                .context()
                .executable_checked_modules(&second_set)
                .is_empty()
        );
        let after_reuse = database.query_trace();
        for name in [
            "body_activation_worklist",
            "executable_checked_module_set",
            "executable_fact_epoch",
            "executable_provider_demands",
            "provider_fact_worklist",
        ] {
            assert_query_executions_unchanged(&before_update, &after_reuse, name);
        }
    }

    #[test]
    fn compiler_input_diff_classifies_provider_fact_resets() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let revision = crate::ProviderFactRevision::new_store();
        let old = CompilerInputs::new(
            CompileRequest::new(fixture.program()).with_provider_fact_revision(revision),
        );
        let cleared = CompilerInputs::new(
            CompileRequest::new(fixture.program()).with_provider_fact_revision(revision.next()),
        );
        let replaced = CompilerInputs::new(
            CompileRequest::new(fixture.program())
                .with_provider_fact_revision(crate::ProviderFactRevision::new_store()),
        );

        assert!(CompilerInputDiff::between(&old, &cleared).provider_facts_reset);
        assert!(CompilerInputDiff::between(&old, &replaced).provider_facts_reset);
    }

    #[test]
    fn compiler_update_rejects_provider_changes_at_unchanged_revision() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let revision = crate::ProviderFactRevision::new_store();
        let database = CompilerDatabase::new(
            CompileRequest::new(fixture.program()).with_provider_fact_revision(revision),
        );
        let demand = crate::ProviderDemand {
            source_path: SourcePath::new("main.nia"),
            request: crate::ProviderRequest::Method {
                target_type_name: None,
                method_name: SymbolId::default(),
            },
        };

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            database.update(
                CompileRequest::new(fixture.program())
                    .with_provider_fact_revision(revision)
                    .with_provider_changes([demand]),
            );
        }));

        assert!(result.is_err());
        assert_eq!(database.provider_fact_revision(), revision);
    }

    #[test]
    #[should_panic(expected = "provider fact owner replacement cannot carry provider changes")]
    fn compiler_input_diff_rejects_provider_changes_from_replacement_owner() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let old = CompilerInputs::new(
            CompileRequest::new(fixture.program())
                .with_provider_fact_revision(crate::ProviderFactRevision::new_store()),
        );
        let demand = crate::ProviderDemand {
            source_path: SourcePath::new("main.nia"),
            request: crate::ProviderRequest::Method {
                target_type_name: None,
                method_name: SymbolId::default(),
            },
        };
        let new = CompilerInputs::new(
            CompileRequest::new(fixture.program())
                .with_provider_fact_revision(crate::ProviderFactRevision::new_store())
                .with_provider_changes([demand]),
        );

        let _ = CompilerInputDiff::between(&old, &new);
    }

    #[test]
    #[should_panic(expected = "provider fact revision cannot move backwards")]
    fn compiler_input_diff_rejects_stale_provider_fact_revision() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let revision = crate::ProviderFactRevision::new_store();
        let old = CompilerInputs::new(
            CompileRequest::new(fixture.program()).with_provider_fact_revision(revision.next()),
        );
        let new = CompilerInputs::new(
            CompileRequest::new(fixture.program()).with_provider_fact_revision(revision),
        );

        let _ = CompilerInputDiff::between(&old, &new);
    }

    #[test]
    fn semantic_provider_activation_preserves_resolved_caller_facts() {
        let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let entry_id = fixture.entry_id();
        let revision = crate::ProviderFactRevision::new_store();
        let database = CompilerDatabase::new(
            CompileRequest::new(fixture.program()).with_provider_fact_revision(revision),
        );
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

        database.update(
            CompileRequest::new(fixture.program())
                .with_provider_fact_revision(revision.next())
                .with_provider_changes([provider_change]),
        );
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
        let database = CompilerDatabase::new(
            CompileRequest::new(fixture.program()).with_provider_fact_revision(revision),
        );
        let provider_changes = database
            .executable_provider_demands()
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
        database.update(
            CompileRequest::new(fixture.program())
                .with_provider_fact_revision(revision.next())
                .with_provider_changes(provider_changes),
        );
        let worklist = database.db.get(ProviderFactWorklistQuery);

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
            .get(DeclarationModuleItemTreeInputQuery(module_id));
        let first_defs = database.db.get(ModuleDefsQuery(module_id));
        assert!(
            first_tree
                .items
                .iter()
                .all(|item| item.node_key.revision == SourceRevision::INITIAL)
        );
        assert!(
            first_defs
                .def_nodes
                .entries()
                .all(|(key, _)| key.revision == SourceRevision::INITIAL)
        );

        fixture.update_module_source(module_id, source, SourceRevision(1));
        let invalidation = database.update(CompileRequest::new(fixture.program()));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(
            invalidated.contains(&"module_source_version"),
            "{invalidated:?}"
        );
        assert!(
            invalidated.contains(&"full_module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"body_check"), "{invalidated:?}");
        assert!(
            invalidated.contains(&"module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"module_defs"), "{invalidated:?}");
        assert!(
            invalidated.contains(&"declaration_type_lowering"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"item_signatures"), "{invalidated:?}");
        assert!(
            invalidated.contains(&"signature_type_lowering"),
            "{invalidated:?}"
        );
        let before_second_check = database.query_trace();

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        let latest_tree = database
            .db
            .get(DeclarationModuleItemTreeInputQuery(module_id));
        let latest_defs = database.db.get(ModuleDefsQuery(module_id));
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
        let first_lowering = database.db.get(TypeLoweringQuery(module_id));
        let type_store = &database.db.context().type_store;
        let first_i32 = type_store
            .append_for_module(module_id)
            .intern(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I32));
        assert!(
            first_lowering
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
        let second_lowering = database.db.get(TypeLoweringQuery(module_id));

        assert_eq!(
            type_store.get(first_i32),
            Some(&nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I32))
        );
        assert!(second_lowering.explicit_type_roots().into_iter().any(|ty| {
            matches!(
                type_store.get(ty),
                Some(nia_ty::TyKind::Pointer { elem, .. })
                    if matches!(
                        type_store.get(*elem),
                        Some(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::Bool))
                    )
            )
        }));
    }

    #[test]
    fn type_normalization_appends_to_the_session_type_store() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "type ByteRef = &u8; pub fn read(value: ByteRef) u8 { 0 }",
        );
        let module_id = fixture.entry_id();
        let database = fixture.database();
        let lowering = database.db.get(TypeLoweringQuery(module_id));
        let normalization = database.db.get(TypeNormalizationQuery(module_id));
        let type_store = &database.db.context().type_store;

        for ty_id in lowering.explicit_type_roots() {
            assert!(type_store.get(ty_id).is_some());
        }
        for normalized in normalization.normalized.values() {
            assert!(type_store.get(*normalized).is_some());
        }
        assert!(
            normalization
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
        let lowering = database.db.get(TypeLoweringQuery(module_id));
        let _ = database.db.get(TypeNormalizationQuery(module_id));

        let _ = database.db.get(ConstArrayLengthsQuery(module_id));
        let _ = database.db.get(ConstEnumValuesQuery(module_id));
        let values = database.db.get(ConstValuesQuery(module_id));
        let _ = database.db.get(ConstTypedFactsQuery(module_id));
        let _ = database.db.get(ConstQuery(module_id));

        for ty in lowering.explicit_type_roots() {
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
        let _ = database.db.get(ConstQuery(module_id));

        let body = database.db.get(BodyCheckQuery(module_id));

        assert!(body.facts.function_facts.values().any(|facts| {
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
                let signature = database.db.get(signature_key);
                let full = database.db.get(TypeNormalizationQuery(module_id));
                (signature, full)
            } else {
                let full = database.db.get(TypeNormalizationQuery(module_id));
                let signature = database.db.get(signature_key);
                (signature, full)
            };

            assert!(
                signature
                    .normalized
                    .values()
                    .chain(full.normalized.values())
                    .all(|ty| database.db.context().type_store.get(*ty).is_some())
            );
            let shared_alias_expansions = signature
                .normalized
                .iter()
                .filter(|(source, normalized)| {
                    source != normalized && full.normalized.get(source) == Some(normalized)
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
        let _ = first.db.get(TypeLoweringQuery(first_module_id));
        let _ = second.db.get(TypeLoweringQuery(second_module_id));
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
    fn function_body_update_refreshes_handles_but_keeps_public_snapshots_green() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "pub struct S { value: i32 } fn main() i32 { 0 }",
        );
        let module_id = fixture.entry_id();
        let database = fixture.database();

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
        let first_using_scope = database.db.get(ModuleUsingScopeQuery(module_id));

        fixture.update_module_source(
            module_id,
            "pub struct S { value: i32 } fn main() i32 { 1 }",
            SourceRevision(1),
        );
        let invalidation = database.update(CompileRequest::new(fixture.program()));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(
            invalidated.contains(&"full_module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(
            invalidated.contains(&"module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"body_check"), "{invalidated:?}");
        assert!(!invalidated.contains(&"loaded_modules"), "{invalidated:?}");
        assert!(invalidated.contains(&"public_surfaces"), "{invalidated:?}");
        assert!(
            invalidated.contains(&"public_using_scopes"),
            "{invalidated:?}"
        );

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        let latest_using_scope = database.db.get(ModuleUsingScopeQuery(module_id));
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
        let invalidation = database.update(CompileRequest::new(fixture.program()));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(
            invalidated.contains(&"full_module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"type_lowering"), "{invalidated:?}");
        assert!(invalidated.contains(&"body_check"), "{invalidated:?}");
        assert!(
            invalidated.contains(&"declaration_type_lowering"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"item_signatures"), "{invalidated:?}");
        assert!(
            !invalidated.iter().any(|name| is_body_signature_query(name)),
            "{invalidated:?}"
        );
        assert!(
            !invalidated.contains(&"extension_methods"),
            "{invalidated:?}"
        );

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
        let invalidation = database.update(CompileRequest::new(fixture.program()));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(invalidated.contains(&"type_lowering"), "{invalidated:?}");
        assert!(
            !invalidated.iter().any(|name| is_body_signature_query(name)),
            "{invalidated:?}"
        );
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
        let invalidation = database.update(CompileRequest::new(fixture.program()));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(
            invalidated.contains(&"declaration_module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(
            invalidated.contains(&"declaration_type_lowering"),
            "{invalidated:?}"
        );
        assert!(
            !invalidated.contains(&"extension_methods"),
            "{invalidated:?}"
        );
        assert!(
            invalidated.contains(&"module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"module_defs"), "{invalidated:?}");
        assert!(invalidated.contains(&"public_surfaces"), "{invalidated:?}");
        assert!(
            invalidated.contains(&"public_using_scopes"),
            "{invalidated:?}"
        );
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
        let invalidation = database.update(CompileRequest::new(fixture.program()));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(invalidated.contains(&"type_lowering"), "{invalidated:?}");
        assert!(
            invalidated.contains(&"declaration_type_lowering"),
            "{invalidated:?}"
        );
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
        let invalidation = database.update(CompileRequest::new(fixture.program()));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(invalidated.contains(&"loaded_modules"), "{invalidated:?}");
        assert!(invalidated.contains(&"checked_module"), "{invalidated:?}");
        assert!(invalidated.contains(&"public_surfaces"), "{invalidated:?}");
        assert!(
            invalidated.contains(&"public_using_scopes"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"module_path"), "{invalidated:?}");

        let second = database.check_program();
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
        let invalidation = database.update(CompileRequest::new(fixture.program()));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(invalidated.contains(&"loaded_modules"), "{invalidated:?}");
        assert!(invalidated.contains(&"module_path"), "{invalidated:?}");
    }

    #[test]
    fn compiler_query_providers_can_override_query_execution() {
        fn no_parse_ok_modules(_: &QueryDb<CompilerContext>) -> StableModuleSequence {
            StableModuleSequence::default()
        }

        let providers = CompilerQueryProviders {
            parse_ok_module_ids: no_parse_ok_modules,
            ..CompilerQueryProviders::default()
        };
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let checked =
            compiler_database_with_providers(CompileRequest::new(fixture.program()), providers)
                .codegen_program();

        assert!(checked.modules.is_empty());
    }

    #[test]
    fn missing_loaded_module_id_becomes_query_diagnostic() {
        fn unknown_module_id() -> ModuleId {
            let mut module_ids = nia_ids::ModuleIdAllocator::new();
            module_ids.allocate();
            module_ids.allocate()
        }

        fn unknown_checked_module(_: &QueryDb<CompilerContext>) -> Vec<ModuleId> {
            vec![unknown_module_id()]
        }

        let providers = CompilerQueryProviders {
            checked_module_ids: unknown_checked_module,
            ..CompilerQueryProviders::default()
        };
        let policy = NiaOptimizationLevel::Oz.policy();
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let checked = compiler_database_with_providers(
            CompileRequest::new(fixture.program()).with_optimization(NiaOptimizationLevel::Oz),
            providers,
        )
        .check_program();

        assert!(checked.modules.is_empty());
        assert_eq!(checked.optimization, policy);
        assert_eq!(checked.diagnostics.len(), 1);
        assert!(
            checked.diagnostics[0]
                .diagnostic
                .summary
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

        let checked = db.get(BodyCheckQuery(entry_id));
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

        let checked = db.get(BodyCheckQuery(entry_id));
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
        let first = database.db.get(ProgramSignatureModuleIdsQuery(
            nia_item_tree::SignatureItemSet::Functions,
        ));
        assert_eq!(
            resolve_stable_module_sequence(&database.db, &first),
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

        let latest = database.db.get(ProgramSignatureModuleIdsQuery(
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
        let first = database.db.get(ProgramSignatureModuleIdsQuery(
            nia_item_tree::SignatureItemSet::Functions,
        ));

        fixture.update_module_source(
            module_id,
            "fn first() i32 { 1 } fn second() i32 { 2 } fn main() i32 { first() }",
            SourceRevision(1),
        );
        database.update(CompileRequest::new(fixture.program()));

        let latest = database.db.get(ProgramSignatureModuleIdsQuery(
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
                &db.get(ProgramSignatureModuleIdsQuery(
                    nia_item_tree::SignatureItemSet::Functions
                ))
            )
            .as_slice(),
            &[module2, module4, module5]
        );
        assert_eq!(
            resolve_stable_module_sequence(
                &db,
                &db.get(ProgramSignatureModuleIdsQuery(
                    nia_item_tree::SignatureItemSet::Values
                ))
            )
            .as_slice(),
            &[module3, module6]
        );
        assert_eq!(
            resolve_stable_module_sequence(
                &db,
                &db.get(ProgramSignatureModuleIdsQuery(
                    nia_item_tree::SignatureItemSet::Types
                ))
            )
            .as_slice(),
            &[module1, module5, module6]
        );
        assert_eq!(
            resolve_stable_module_sequence(
                &db,
                &db.get(ProgramSignatureModuleIdsQuery(
                    nia_item_tree::SignatureItemSet::Traits
                ))
            )
            .as_slice(),
            &[module4, module5, module6]
        );
        assert_eq!(
            resolve_stable_module_sequence(
                &db,
                &db.get(ProgramSignatureModuleIdsQuery(
                    nia_item_tree::SignatureItemSet::ExtensionFunctions
                ))
            )
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
            resolve_stable_module_sequence(&db, &db.get(ExtensionProviderModuleIdsQuery))
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

        let defs = db.get(ModuleDefsQuery(module_id));
        let alias_id = defs.module_scope.types.get(&sym("Alias")).unwrap();
        let _ = db.get(ProgramTypeAliasSignatureQuery(GlobalDefId {
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

        let layouts = db.get(LayoutsQuery(module_id));
        let trace = db.query_trace();

        assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
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

        let layouts = db.get(LayoutsQuery(entry_id));
        let trace = db.query_trace();
        let entry_description = format!("{entry_id:?}");
        let module1_description = format!("{module1:?}");

        assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
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

        let _ = db.get(SignatureTypeNormalizationQuery(entry_id, signature_types));
        let _ = db.get(SignatureItemSignaturesQuery(entry_id, signature_types));
        let layouts = db.get(SignatureLayoutsQuery(entry_id));

        assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
        assert!(
            layouts
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

        let _ = db.get(AbiCheckQuery(module_id));
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

        let _ = db.get(PublicSurfacesQuery);
        let _ = db.get(ModulePublicSurfaceQuery(module_id));
        let _ = db.get(ModuleUsingScopeQuery(module_id));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "public_surfaces" && dependency.to.name == "module_defs"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "public_surfaces" && dependency.to.name == "module_graph"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "public_using_scopes" && dependency.to.name == "public_surfaces"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "public_using_scopes" && dependency.to.name == "module_defs"
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

        let module_input: Arc<ModuleItemTree> = db.get(ModuleItemTreeInputQuery(module_id));
        let active_input: Arc<ActiveModuleItemTree> =
            db.get(ActiveModuleItemTreeInputQuery(module_id));
        let full_module: Arc<ModuleItemTree> = db.get(FullModuleItemTreeQuery(module_id));
        let full_active: Arc<ActiveModuleItemTree> =
            db.get(FullActiveModuleItemTreeQuery(module_id));

        let module_input_batch = db.get_many([ModuleItemTreeInputQuery(module_id)]);
        let active_input_batch = db.get_many([ActiveModuleItemTreeInputQuery(module_id)]);
        let full_module_batch = db.get_many([FullModuleItemTreeQuery(module_id)]);
        let full_active_batch = db.get_many([FullActiveModuleItemTreeQuery(module_id)]);

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
        let defs = db.get(ModuleDefsQuery(module_id));
        let main = GlobalDefId {
            module_id,
            def_id: defs.module_scope.values.get(&sym("main")).unwrap(),
        };
        let helper = GlobalDefId {
            module_id,
            def_id: defs.module_scope.values.get(&sym("helper")).unwrap(),
        };

        let edges = db.get(ExecutableValueRefEdgesQuery(main));
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
    fn executable_value_ref_item_refreshes_from_current_module_facts() {
        let source = "fn helper() i32 { 1 } fn main() i32 { helper() }";
        let mut fixture = LoadedProgramFixture::new("main.nia", source);
        let module_id = fixture.entry_id();
        let database = fixture.database();
        let defs = database.db.get(ModuleDefsQuery(module_id));
        let owner = GlobalDefId {
            module_id,
            def_id: defs.module_scope.values.get(&sym("main")).unwrap(),
        };
        let first = database.db.get(ExecutableValueRefItemQuery(owner));
        assert_eq!(
            first.as_ref().as_ref().unwrap().owner_node_key.revision,
            SourceRevision::INITIAL
        );

        fixture.update_module_source(module_id, source, SourceRevision(1));
        let invalidation = database.update(CompileRequest::new(fixture.program()));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();
        assert!(
            invalidated.contains(&"executable_value_ref_item"),
            "{invalidated:?}"
        );

        let latest = database.db.get(ExecutableValueRefItemQuery(owner));
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
        let defs = db.get(ModuleDefsQuery(module_id));
        let main = GlobalDefId {
            module_id,
            def_id: defs.module_scope.values.get(&sym("main")).unwrap(),
        };
        let calls = GlobalDefId {
            module_id,
            def_id: defs.module_scope.values.get(&sym("calls")).unwrap(),
        };

        let edges = db.get(ExecutableValueRefEdgesQuery(main));

        assert!(edges.globals.contains(&calls), "{:?}", edges.globals);
    }

    #[test]
    fn module_defs_query_uses_active_item_tree_query() {
        let fixture = LoadedProgramFixture::new("main.nia", "pub struct S { value: i32 }");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let defs = db.get(ModuleDefsQuery(module_id));
        let item_tree = db.get(ActiveModuleItemTreeQuery(module_id));
        let item_node_key = &item_tree.items[0].node_key;
        let item_node_id = defs
            .def_nodes
            .node_id(item_node_key)
            .expect("definition node id");
        let trace = db.query_trace();

        assert_eq!(defs.def_nodes.store_id(), db.context().node_store.id());
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

        let _ = db.get(ExtensionProviderValidationFactsQuery(module_id));
        let _ = db.get(ExtensionProviderModuleFactsQuery(module_id));
        let _ = db.get(ExtensionMethodIndexQuery);
        let _ = db.get(ExtensionProviderDiscoveryIndexQuery);
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

        let _ = database.db.get(ExtensionMethodIndexQuery);
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
        let invalidation = database.update(CompileRequest::new(fixture.program()));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(
            invalidated.contains(&"extension_provider_module_facts"),
            "{invalidated:?}"
        );
        assert!(
            !invalidated.contains(&"extension_provider_summary"),
            "{invalidated:?}"
        );
        let before_second_query = database.query_trace();

        let _ = database.db.get(ExtensionMethodIndexQuery);
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
            .get(ExtensionProviderModuleEligibilityQuery(module_id));
        assert!(*first);
        let first_modules = database.db.get(ExtensionProviderModuleIdsQuery);

        fixture.update_module_source(
            module_id,
            "struct S {} extend S { pub fn first() i32 { 1 } pub fn second() i32 { 2 } }",
            SourceRevision(1),
        );
        let invalidation = database.update(CompileRequest::new(fixture.program()));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();
        assert!(invalidated.contains(&"extension_provider_summary"));
        assert!(invalidated.contains(&"extension_provider_module_eligibility"));

        let second = database
            .db
            .get(ExtensionProviderModuleEligibilityQuery(module_id));
        assert!(*second);
        assert!(!Arc::ptr_eq(&first, &second));
        let latest_modules = database.db.get(ExtensionProviderModuleIdsQuery);
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

        let _ = db.get(ValueResolutionQuery(module_id));
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

        let values = db.get(ValueResolutionQuery(entry_id));
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

        let values = db.get(ValueResolutionQuery(module_id));
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

        let _ = db.get(FlowCheckQuery(module_id));
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

        let _ = db.get(StaticCheckQuery(module_id));
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

        let _ = db.get(BodyCheckQuery(module_id));
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

        let checked = db.get(BodyCheckQuery(module_id));

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

        let _ = db.get(VisibleExtensionsQuery(entry_id));
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

        let _ = db.get(ConstQuery(module_id));
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

        let _ = db.get(MonomorphizationQuery);
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

        let _ = db.get(ExecutableCheckedModulesQuery);
        let trace = db.query_trace();

        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_checked_module_set"
                && dependency.to.name == "program_executable_reachability_signatures"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_checked_module_set"
                && dependency.to.name == "signature_item_signatures"
        }));
        assert!(!depends_on_body_signature_query(
            &trace,
            "executable_checked_module_set"
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

        let checked = db.get(BodyCheckQuery(module_id));
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

        let checked = db.get(BodyCheckQuery(module_id));
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

        let checked = db.get(CodegenProgramQuery);
        let trace = db.query_trace();

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_checked_module_set"
                && dependency.to.name == "extension_trait_impls_for_trait"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_checked_module_set"
                && dependency.to.name == "program_trait_solving_signatures"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_checked_module_set"
                && dependency.to.name == "extension_provider_module_facts"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_checked_module_set"
                && dependency.to.name == "extension_method_index"
        }));
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

        let checked = db.get(EntryCheckedProgramQuery);
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
                && dependency.to.name == "executable_checked_module_set"
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

        let checked = db.get(EntryCheckedProgramQuery);
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
                && dependency.to.name == "executable_checked_module_set"
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

        let checked = db.get(CodegenProgramQuery);

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

        let checked = db.get(CodegenProgramQuery);

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

        let trait_impls = db.get(VisibleTraitImplsQuery(entry_id));

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

        let checked = db.get(ExecutableCheckedModulesQuery);
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

        let _ = db.get(ConstModuleQuery(module_id));
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

        let table = db.get(SemanticUseTableQuery(module_id));
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

        let values = db.get(ValueResolutionQuery(module_id));
        assert_eq!(values.node_names.store_id(), node_store_id);
        assert_eq!(values.node_qualified_values.store_id(), node_store_id);
        assert_eq!(
            values.node_builtin_associated_values.store_id(),
            node_store_id
        );
        assert_eq!(values.node_variant_enums.store_id(), node_store_id);
        assert_eq!(
            values.node_qualified_type_prefixes.store_id(),
            node_store_id
        );

        let locals = db.get(LocalResolutionQuery(module_id));
        assert_eq!(locals.node_local_defs.store_id(), node_store_id);
        assert_eq!(locals.node_uses.store_id(), node_store_id);

        let types = db.get(TypeResolutionQuery(module_id));
        assert_eq!(types.node_const_generic_names.store_id(), node_store_id);
    }

    #[test]
    fn checked_module_exposes_semantic_use_table_product() {
        let source = "fn main() i32 { let mut local: i32 = 1; local }";
        let fixture = LoadedProgramFixture::new("main.nia", source);
        let checked = fixture.database().check_program();

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

        let checked = db.get(CheckedModuleQuery(module_id));
        let checked_program = db.get(CheckedProgramQuery);
        let values = db.get(ValueResolutionQuery(module_id));
        let locals = db.get(LocalResolutionQuery(module_id));
        let semantic_uses = db.get(SemanticUseTableQuery(module_id));
        let type_resolution = db.get(TypeResolutionQuery(module_id));
        let type_lowering = db.get(TypeLoweringQuery(module_id));
        let type_normalization = db.get(TypeNormalizationQuery(module_id));
        let layouts = db.get(LayoutsQuery(module_id));
        let body_check = db.get(BodyCheckQuery(module_id));
        let const_eval = db.get(ConstQuery(module_id));
        let const_array_lengths = db.get(ConstArrayLengthsQuery(module_id));
        let const_enum_values = db.get(ConstEnumValuesQuery(module_id));
        let const_values = db.get(ConstValuesQuery(module_id));
        let const_typed_facts = db.get(ConstTypedFactsQuery(module_id));
        let static_check = db.get(StaticCheckQuery(module_id));
        let abi_check = db.get(AbiCheckQuery(module_id));
        let flow_check = db.get(FlowCheckQuery(module_id));

        assert!(Arc::ptr_eq(&checked, &checked_program.modules[0]));
        assert!(Arc::ptr_eq(&checked.value_resolution, &values));
        assert!(Arc::ptr_eq(&checked.local_resolution, &locals));
        assert!(Arc::ptr_eq(&checked.semantic_uses, &semantic_uses));
        assert!(Arc::ptr_eq(&checked.type_resolution, &type_resolution));
        assert!(Arc::ptr_eq(&checked.type_lowering, &type_lowering));
        assert!(Arc::ptr_eq(
            &checked.type_normalization,
            &type_normalization
        ));
        assert!(Arc::ptr_eq(&checked.layouts, &layouts));
        assert!(Arc::ptr_eq(&checked.body_ir, &body_check.ir));
        assert!(Arc::ptr_eq(&checked.semantic_facts, &body_check.facts));
        assert!(Arc::ptr_eq(
            &checked.provider_demands,
            &body_check.provider_demands
        ));
        assert!(Arc::ptr_eq(
            &checked.body_diagnostics,
            &body_check.diagnostics
        ));
        assert!(Arc::ptr_eq(&checked.const_eval, &const_eval));
        assert!(Arc::ptr_eq(&const_eval.values, &const_values.values));
        assert!(Arc::ptr_eq(
            &const_eval.typed_values,
            &const_typed_facts.typed_values
        ));
        assert!(Arc::ptr_eq(
            &const_eval.enum_values,
            &const_enum_values.values
        ));
        assert!(Arc::ptr_eq(
            &const_eval.typed_enum_values,
            &const_enum_values.typed_values
        ));
        assert!(Arc::ptr_eq(
            &const_eval.array_lengths,
            &const_array_lengths.values
        ));
        assert!(Arc::ptr_eq(&checked.static_check, &static_check));
        assert!(Arc::ptr_eq(&checked.abi_check, &abi_check));
        assert!(Arc::ptr_eq(&checked.flow_check, &flow_check));
    }

    #[test]
    fn program_products_share_the_input_module_graph_snapshot() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let loaded = fixture.program();
        let input_graph = loaded.graph.clone();
        let db = query_db(loaded);

        let cached_graph = db.get(ModuleGraphQuery);
        let checked = db.get(CheckedProgramQuery);
        let codegen = db.get(CodegenProgramQuery);

        assert!(input_graph.ptr_eq(&cached_graph));
        assert!(input_graph.ptr_eq(&checked.graph));
        assert!(input_graph.ptr_eq(&codegen.graph));
    }

    #[test]
    fn checked_modules_reuse_cached_definition_handles() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 1 }");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let defs = db.get(FullModuleDefsQuery(module_id));
        let checked = db.get(CheckedModuleQuery(module_id));
        let executable = db.get(ExecutableCheckedModulesQuery);
        let executable = executable
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry executable module");

        assert!(Arc::ptr_eq(&checked.defs, &defs));
        assert!(Arc::ptr_eq(&executable.defs, &defs));
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

        let signature = db.get(ModuleProgramSignatureFactsQuery(module_id, signature_set));
        let abi = db.get(ModuleAbiSignatureFactsQuery(module_id));
        let trait_solving = db.get(ExtensionTraitSolvingModuleFactsQuery(module_id));
        let provider = db.get(ExtensionProviderModuleFactsQuery(module_id));
        let nominal = db.get(ExtensionProviderNominalModuleFactsQuery(module_id));
        let visible_extensions: Arc<VisibleExtensionsForModule> =
            db.get(VisibleExtensionsQuery(module_id));
        let visible_trait_impls: Arc<VisibleTraitImplsForModule> =
            db.get(VisibleTraitImplsQuery(module_id));
        let trait_method_index: Arc<nia_program_signatures::ProgramTraitMethodIndex> =
            db.get(ProgramTraitMethodIndexQuery);
        let abi_signatures: Arc<ProgramAbiSignaturesValue> = db.get(ProgramAbiSignaturesQuery);

        let signature_batch =
            db.get_many([ModuleProgramSignatureFactsQuery(module_id, signature_set)]);
        let abi_batch = db.get_many([ModuleAbiSignatureFactsQuery(module_id)]);
        let trait_solving_batch = db.get_many([ExtensionTraitSolvingModuleFactsQuery(module_id)]);
        let provider_batch = db.get_many([ExtensionProviderModuleFactsQuery(module_id)]);
        let nominal_batch = db.get_many([ExtensionProviderNominalModuleFactsQuery(module_id)]);
        let visible_extensions_batch = db.get_many([VisibleExtensionsQuery(module_id)]);
        let visible_trait_impls_batch = db.get_many([VisibleTraitImplsQuery(module_id)]);
        let trait_method_index_batch = db.get_many([ProgramTraitMethodIndexQuery]);
        let abi_signatures_batch = db.get_many([ProgramAbiSignaturesQuery]);

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
        let defs = db.get(ModuleDefsQuery(module_id));
        let trait_id = nia_ty::TraitId::Source(GlobalDefId {
            module_id,
            def_id: defs.module_scope.types.get(&sym("Read")).unwrap(),
        });
        let method_id = GlobalDefId {
            module_id,
            def_id: nia_ids::DefId(0),
        };

        let validation: Arc<ExtensionProviderValidationFactsQueryValue> =
            db.get(ExtensionProviderValidationFactsQuery(module_id));
        let discovery: Arc<ExtensionProviderDiscoveryIndexQueryValue> =
            db.get(ExtensionProviderDiscoveryIndexQuery);
        let exposure: Arc<TypeExposureIndex> = db.get(TypeExposureIndexQuery);
        let methods: Arc<ExtensionMethodIndexQueryValue> = db.get(ExtensionMethodIndexQuery);
        let named: Arc<ExtensionMethodsNamedQueryValue> =
            db.get(ExtensionMethodsNamedQuery(sym("get")));
        let method: Arc<ExtensionMethodByIdQueryValue> =
            db.get(ExtensionMethodByIdQuery(method_id));
        let trait_index: Arc<ExtensionTraitSignatureIndex> =
            db.get(ExtensionTraitSignatureIndexQuery);
        let signature_input: Arc<ExtensionSignatureModuleInputQueryValue> =
            db.get(ExtensionSignatureModuleInputQuery(module_id));
        let trait_impls: Arc<ExtensionTraitImplsForTraitQueryValue> =
            db.get(ExtensionTraitImplsForTraitQuery(trait_id));

        let validation_batch = db.get_many([ExtensionProviderValidationFactsQuery(module_id)]);
        let discovery_batch = db.get_many([ExtensionProviderDiscoveryIndexQuery]);
        let exposure_batch = db.get_many([TypeExposureIndexQuery]);
        let methods_batch = db.get_many([ExtensionMethodIndexQuery]);
        let named_batch = db.get_many([ExtensionMethodsNamedQuery(sym("get"))]);
        let method_batch = db.get_many([ExtensionMethodByIdQuery(method_id)]);
        let trait_index_batch = db.get_many([ExtensionTraitSignatureIndexQuery]);
        let signature_input_batch = db.get_many([ExtensionSignatureModuleInputQuery(module_id)]);
        let trait_impls_batch = db.get_many([ExtensionTraitImplsForTraitQuery(trait_id)]);

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

        let surfaces: Arc<PublicSurfacesQueryValue> = db.get(PublicSurfacesQuery);
        let using_scopes: Arc<PublicUsingScopesQueryValue> = db.get(PublicUsingScopesQuery);
        let module_using_scope: Arc<ModuleUsingScope> = db.get(ModuleUsingScopeQuery(module_id));

        let surfaces_batch = db.get_many([PublicSurfacesQuery]);
        let using_scopes_batch = db.get_many([PublicUsingScopesQuery]);
        let module_using_scope_batch = db.get_many([ModuleUsingScopeQuery(module_id)]);

        assert!(Arc::ptr_eq(&surfaces, &surfaces_batch[0]));
        assert!(Arc::ptr_eq(&using_scopes, &using_scopes_batch[0]));
        assert!(Arc::ptr_eq(
            &module_using_scope,
            &module_using_scope_batch[0]
        ));
    }

    #[test]
    fn backend_lowering_uses_executable_checked_module_body_ir() {
        let fixture =
            LoadedProgramFixture::new("main.nia", "fn main() i32 { static value: i32 = 1; value }");
        let db = query_db(fixture.program());

        let _ = db.get(BackendLoweringQuery);
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering"
                && dependency.to.name == "executable_checked_module_set"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering"
                && dependency.to.name == "full_active_module_item_tree"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering" && dependency.to.name == "checked_module_ids"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering"
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
    fn codegen_reuses_lowered_function_bodies_between_mono_and_backend() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "fn helper() i32 { 1 } fn main() i32 { helper() }",
        );
        let db = query_db(fixture.program());

        let codegen = db.get(CodegenProgramQuery);
        let trace = db.query_trace();

        assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
        assert_eq!(
            query_executions(&trace, "lowered_function_bodies"),
            codegen.modules.len(),
            "function lowering should execute once per executable module"
        );
        assert!(
            query_cache_hits(&trace, "lowered_function_bodies") >= codegen.modules.len(),
            "backend lowering should reuse monomorphization's function products"
        );
        assert!(trace_has_dependency(
            &trace,
            "codegen_program",
            "lowered_function_bodies"
        ));
    }

    #[test]
    fn codegen_public_adapter_reuses_large_product_handles() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 1 }");
        let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));

        let cached = database.db.get(CodegenProgramQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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
            dependency.from.name == "executable_checked_module_set"
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let signatures = db.get(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let impl_signature = signatures
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

        let signatures = db.get(SignatureItemSignaturesQuery(
            entry_id,
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let impl_signature = signatures
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

        let signatures = db.get(SignatureItemSignaturesQuery(
            entry_id,
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let impl_signature = signatures
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == module_id)
            .expect("entry module should be executable-reachable");
        assert!(
            module.layouts.diagnostics.is_empty(),
            "unreachable recursive aggregate should not force layout diagnostics: {:?}",
            module.layouts.diagnostics
        );

        let backend_lowering = db.get(BackendLoweringQuery);
        let backend_module = backend_lowering
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

        let backend_lowering = db.get(BackendLoweringQuery);

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
        fixture.add_child(
            entry_id,
            "module1",
            "module1.nia",
            r#"
pub trait Allocator {
    fn alloc(&mut self) i32;

    fn remap(&mut self) i32 {
        self.alloc()
    }
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
        let db = query_db(loaded);

        let backend_lowering = db.get(BackendLoweringQuery);

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
                        &db.context().type_store,
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let backend = db.get(BackendLoweringQuery);
        let backend_module = backend
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let modules = db.get(ExecutableCheckedModulesQuery);
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

        let _ = db.get(BodyCheckQuery(module_id));
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

        let _ = db.get(TypeResolutionQuery(module_id));
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

        let _ = db.get(TypeResolutionQuery(module_id));
    }

    #[test]
    fn invalidates_module_defs_after_item_tree_changes() {
        let fixture = LoadedProgramFixture::new("main.nia", "pub struct S { value: i32 }");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let _ = db.get(ModuleDefsQuery(module_id));
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
