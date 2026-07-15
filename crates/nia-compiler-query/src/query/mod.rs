// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    CheckedModule, CheckedProgram, CodegenProgram, LoadedModule, LoadedProgram, ProgramDiagnostic,
    RuntimeModel, TimingMode, module_diagnostics,
};
use nia_backend_lower::BackendLowerModuleInput;
use nia_const_check::{ConstCheck, ConstModuleLowering};
use nia_defs::{
    DefCollection, ModulePublicSurface, ModuleUsingScope, PublicSurfaceLookup, PublicSurfaces,
    UsingScopeLookup,
};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId};
use nia_imports::{ModuleGraph, ModuleGraphLookup, ModuleNode, ModuleNodeRef};
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
use nia_query::{QueryDb, QueryError, QueryFrame, QueryKey, QueryTrace};
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

type ExtensionProviderModuleFactsValue = Arc<ExtensionProviderModuleFactsQueryValue>;
type ExtensionProviderValidationFactsValue = Arc<ExtensionProviderValidationFactsQueryValue>;
type ExtensionProviderNominalModuleFactsValue = Arc<ExtensionProviderNominalModuleFactsQueryValue>;
type ExtensionProviderDiscoveryIndexValue = Arc<ExtensionProviderDiscoveryIndexQueryValue>;
type ExtensionProviderNominalCandidateModulesValue =
    Arc<ExtensionProviderNominalCandidateModulesQueryValue>;
type ExtensionProviderNominalModulesForTargetsValue =
    Arc<ExtensionProviderNominalModulesForTargetsQueryValue>;
type TypeExposureIndexValue = Arc<TypeExposureIndex>;
type ExtensionMethodIndexValue = Arc<ExtensionMethodIndexQueryValue>;
type ExtensionMethodsNamedValue = Arc<ExtensionMethodsNamedQueryValue>;
type ExtensionMethodByIdValue = Arc<ExtensionMethodByIdQueryValue>;
type ExtensionTraitSignatureIndexValue = Arc<ExtensionTraitSignatureIndex>;
type VisibleExtensionsValue = Arc<VisibleExtensionsForModule>;
type VisibleTraitImplsValue = Arc<VisibleTraitImplsForModule>;
type ExtensionSignatureModuleInputValue = Arc<ExtensionSignatureModuleInputQueryValue>;
type ExtensionTraitSolvingModuleFactsValue = Arc<ExtensionTraitSolvingModuleFactsQueryValue>;
type ExtensionTraitImplsForTraitValue = Arc<ExtensionTraitImplsForTraitQueryValue>;
type ModuleProgramSignatureFactsValue = Arc<ModuleProgramSignatureFacts>;
type ModuleAbiSignatureFactsValue = Arc<ModuleAbiSignatureFactsQueryValue>;
type PublicSurfacesValue = Arc<PublicSurfacesQueryValue>;
type PublicUsingScopesValue = Arc<PublicUsingScopesQueryValue>;

#[derive(Debug, Clone)]
pub struct CompileRequest {
    pub loaded: LoadedProgram,
    pub optimization: NiaOptimizationLevel,
    pub timings: TimingMode,
    pub provider_changes: Vec<crate::ProviderDemand>,
}

impl CompileRequest {
    pub fn new(loaded: LoadedProgram) -> Self {
        Self {
            loaded,
            optimization: NiaOptimizationLevel::default(),
            timings: TimingMode::Off,
            provider_changes: Vec::new(),
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

    pub fn check_program(&self) -> CheckedProgram {
        #[cfg(test)]
        let _permit = nia_test_support::compiler_permit();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.db.try_query(CheckedProgramQuery)
        })) {
            Ok(Ok(checked)) => checked,
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
            self.db.try_query(EntryCheckedProgramQuery)
        })) {
            Ok(Ok(checked)) => checked,
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
            .try_query(ExecutableProviderDemandsQuery)
            .unwrap_or_default()
    }

    pub fn codegen_program(&self) -> CodegenProgram {
        #[cfg(test)]
        let _permit = nia_test_support::compiler_permit();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.db.try_query(CodegenProgramQuery)
        })) {
            Ok(Ok(codegen)) => codegen,
            Ok(Err(err)) => codegen_program_from_query_error(
                self.current_graph(),
                self.current_optimization(),
                err,
            ),
            Err(payload) => match payload.downcast::<QueryError>() {
                Ok(err) => codegen_program_from_query_error(
                    self.current_graph(),
                    self.current_optimization(),
                    *err,
                ),
                Err(payload) => std::panic::resume_unwind(payload),
            },
        }
    }

    pub fn update(&self, request: CompileRequest) -> CompilerInvalidation {
        let new_inputs = CompilerInputs::new(request);
        let diff = {
            let mut inputs = self.inputs.write().expect("compiler input lock poisoned");
            let diff = CompilerInputDiff::between(&inputs, &new_inputs);
            *inputs = new_inputs;
            diff
        };
        self.invalidate_inputs(diff)
    }

    pub fn query_trace(&self) -> QueryTrace {
        self.db.query_trace()
    }

    fn current_graph(&self) -> ModuleGraph {
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
        let reset_executable_facts = diff.executable_fact_inputs_changed
            || diff.target_changed
            || diff.runtime_changed
            || diff.executable_roots_changed;
        if reset_executable_facts {
            self.db.context().clear_executable_fact_session();
        } else if diff.graph_changed {
            self.db
                .context()
                .retain_executable_facts_after_graph_growth(
                    &diff.body_activated_modules,
                    &diff.provider_changes,
                );
        }
        self.db.context().clear_executable_checked_module_sets();
        let mut invalidation = CompilerInvalidation::default();
        if diff.graph_entry_changed {
            invalidation.extend(self.db.invalidate(ModuleGraphEntryQuery));
        }
        for module_id in diff.changed_graph_modules {
            invalidation.extend(self.db.invalidate(ModuleGraphNodeQuery(module_id)));
        }
        for module_id in diff.changed_graph_paths {
            invalidation.extend(self.db.invalidate(ModuleGraphPathQuery(module_id)));
        }
        for module_id in diff.changed_graph_parents {
            invalidation.extend(self.db.invalidate(ModuleGraphParentQuery(module_id)));
        }
        for (module_id, name) in diff.changed_graph_children {
            invalidation.extend(self.db.invalidate(ModuleGraphChildQuery(module_id, name)));
        }
        for package in diff.changed_package_roots {
            invalidation.extend(self.db.invalidate(ModulePackageRootQuery(package)));
        }
        if diff.graph_changed {
            invalidation.extend(self.db.invalidate(ModuleGraphQuery));
            invalidation.extend(self.db.invalidate(SemanticModuleIdsQuery));
        }
        if diff.public_surfaces_changed {
            invalidation.extend(self.db.invalidate(PublicSurfacesQuery));
        }
        for module_id in diff.changed_public_surface_modules {
            invalidation.extend(self.db.invalidate(ModulePublicSurfaceQuery(module_id)));
        }
        for (module_id, name) in diff.changed_public_surface_module_names {
            invalidation.extend(
                self.db
                    .invalidate(PublicSurfaceModuleQuery(module_id, name)),
            );
        }
        for (module_id, name) in diff.changed_public_surface_value_names {
            invalidation.extend(self.db.invalidate(PublicSurfaceValueQuery(module_id, name)));
        }
        for (module_id, name) in diff.changed_public_surface_type_names {
            invalidation.extend(self.db.invalidate(PublicSurfaceTypeQuery(module_id, name)));
        }
        if diff.public_using_scopes_changed {
            invalidation.extend(self.db.invalidate(PublicUsingScopesQuery));
        }
        for module_id in diff.changed_public_using_scope_modules {
            invalidation.extend(self.db.invalidate(ModuleUsingScopeQuery(module_id)));
        }
        for (module_id, name) in diff.changed_using_scope_module_names {
            invalidation.extend(self.db.invalidate(UsingScopeModuleQuery(module_id, name)));
        }
        for (module_id, name) in diff.changed_using_scope_value_names {
            invalidation.extend(self.db.invalidate(UsingScopeValueQuery(module_id, name)));
        }
        for (module_id, name) in diff.changed_using_scope_type_names {
            invalidation.extend(self.db.invalidate(UsingScopeTypeQuery(module_id, name)));
        }
        for (module_id, name) in diff.changed_using_scope_unresolved_names {
            invalidation.extend(
                self.db
                    .invalidate(UsingScopeUnresolvedQuery(module_id, name)),
            );
        }
        for owner in diff.changed_executable_value_ref_items {
            invalidation.extend(self.db.invalidate(ExecutableValueRefItemQuery(owner)));
        }
        if diff.executable_roots_changed {
            invalidation.extend(self.db.invalidate(ExecutableRootModulesQuery));
        }
        if diff.loaded_modules_changed {
            invalidation.extend(self.db.invalidate(LoadedModulesQuery));
            invalidation.extend(self.db.invalidate(SemanticModuleIdsQuery));
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
        if diff.optimization_changed {
            invalidation.extend(self.db.invalidate(CompilerOptimizationQuery));
        }
        for module in diff.changed_modules {
            for module_id in module.ids {
                if module.path {
                    invalidation.extend(self.db.invalidate(ModulePathQuery(module_id)));
                }
                if module.source_version {
                    invalidation.extend(self.db.invalidate(ModuleSourceVersionQuery(module_id)));
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
                    invalidation.extend(self.db.invalidate(SemanticModuleIdsQuery));
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
                    invalidation
                        .extend(self.db.invalidate(ExtensionProviderSummaryQuery(module_id)));
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
    let timings = request.timings;
    let inputs = Arc::new(RwLock::new(CompilerInputs::new(request)));
    let executable_checked_modules = Arc::new(RwLock::new(ExecutableCheckedModuleStore::default()));
    let executable_fact_session = Arc::new(std::sync::Mutex::new(ExecutableFactSession::default()));
    let type_store = Arc::new(nia_ty::TypeStore::new());
    let db = QueryDb::new_with_timings(
        CompilerContext {
            inputs: inputs.clone(),
            providers,
            executable_checked_modules,
            executable_fact_session,
            type_store,
        },
        timings,
    );
    CompilerDatabase { db, inputs }
}

fn checked_program_from_query_error(
    graph: ModuleGraph,
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
    graph: ModuleGraph,
    optimization: OptimizationPolicy,
    err: QueryError,
) -> CodegenProgram {
    CodegenProgram {
        graph,
        optimization,
        modules: Vec::new(),
        monomorphization: nia_monomorphize::Monomorphization {
            instances: Vec::new(),
            diagnostics: Vec::new(),
        },
        backend_lowering: nia_backend_lower::BackendLowering {
            program: nia_backend_ir::BackendProgram {
                modules: Vec::new(),
            },
            optimization,
            optimization_report: nia_backend_lower::BackendOptimizationReport::default(),
            diagnostics: Vec::new(),
        },
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
    providers: CompilerQueryProviders,
    executable_checked_modules: Arc<RwLock<ExecutableCheckedModuleStore>>,
    executable_fact_session: Arc<std::sync::Mutex<ExecutableFactSession>>,
    type_store: Arc<nia_ty::TypeStore>,
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
    modules: HashMap<ModuleId, CheckedModule>,
}

#[derive(Debug, Clone)]
struct CompilerInputs {
    graph: ModuleGraph,
    entry_module: ModuleId,
    runtime_root_modules: Vec<ModuleId>,
    symbols: nia_symbol_table::SymbolTable,
    modules: Vec<CompilerInputModule>,
    modules_by_id: HashMap<ModuleId, usize>,
    modules_by_source_identity: HashMap<SourceIdentity, usize>,
    public_surfaces: PublicSurfacesValue,
    public_using_scopes: PublicUsingScopesValue,
    executable_value_ref_items: HashMap<GlobalDefId, ExecutableValueRefItemLocation>,
    diagnostics: Vec<ProgramDiagnostic>,
    target: TargetConfig,
    runtime: crate::RuntimeModel,
    optimization: OptimizationPolicy,
    timings: TimingMode,
    provider_changes: Vec<crate::ProviderDemand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExecutableValueRefItemLocation {
    module_id: ModuleId,
    item_index: usize,
    owner_node_key: nia_node_id::VersionedNodeKey,
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
        let loaded = request.loaded;
        validate_loaded_module_identities(&loaded);
        let graph = loaded.graph;
        let entry_module = graph.entry();
        let runtime_root_modules = graph
            .modules()
            .filter(|node| graph.is_executable_root_module(node.id))
            .map(|node| node.id)
            .collect();
        let symbols = loaded.symbols;
        let target = loaded.target;
        let runtime = loaded.runtime;
        let diagnostics = loaded.diagnostics;
        let provider_changes = request.provider_changes;
        let modules = loaded
            .modules
            .into_iter()
            .map(CompilerInputModule::from_loaded)
            .collect::<Vec<_>>();
        let modules_by_id = index_input_modules(&modules);
        let modules_by_source_identity = index_input_module_identities(&modules);
        let defs = modules
            .iter()
            .filter(|module| module.parse_errors.is_empty())
            .map(|module| {
                nia_defs::collect_module_defs_from_active_item_tree_with_symbols(
                    module.id,
                    &module.active_item_tree,
                    &symbols,
                )
            })
            .collect::<Vec<_>>();
        let exports = compute_exported_public_surfaces_with_symbols(&defs, &graph, &symbols);
        let using_scopes = compute_using_scopes_from_surfaces_with_symbols(
            &defs,
            &graph,
            &exports.surfaces,
            &symbols,
        );
        let public_surfaces = Arc::new(PublicSurfacesQueryValue {
            surfaces: exports.surfaces,
            diagnostics: exports.diagnostics,
        });
        let public_using_scopes = Arc::new(PublicUsingScopesQueryValue {
            using_scopes: using_scopes.using_scopes,
            diagnostics: using_scopes.diagnostics,
        });
        let executable_value_ref_items = index_executable_value_ref_items(&modules, &defs);
        Self {
            graph,
            entry_module,
            runtime_root_modules,
            symbols,
            modules,
            modules_by_id,
            modules_by_source_identity,
            public_surfaces,
            public_using_scopes,
            executable_value_ref_items,
            diagnostics,
            target,
            runtime,
            optimization: request.optimization.policy(),
            timings: request.timings,
            provider_changes,
        }
    }
}

fn index_executable_value_ref_items(
    modules: &[CompilerInputModule],
    defs_by_module: &[DefCollection],
) -> HashMap<GlobalDefId, ExecutableValueRefItemLocation> {
    let defs_by_id = defs_by_module
        .iter()
        .map(|defs| (defs.module_id, defs))
        .collect::<HashMap<_, _>>();
    let mut items = HashMap::new();
    for module in modules {
        let Some(defs) = defs_by_id.get(&module.id).copied() else {
            continue;
        };
        for (item_index, item) in module.active_item_tree.items.iter().enumerate() {
            index_executable_value_ref_item(module.id, item_index, item, defs, &mut items);
        }
    }
    items
}

fn index_executable_value_ref_item(
    module_id: ModuleId,
    item_index: usize,
    item: &nia_item_tree::ItemTreeNode,
    defs: &DefCollection,
    items: &mut HashMap<GlobalDefId, ExecutableValueRefItemLocation>,
) {
    let mut insert = |node_key: &nia_node_id::VersionedNodeKey| {
        let Some(def_id) = defs.def_nodes.get(node_key) else {
            return;
        };
        items.insert(
            GlobalDefId { module_id, def_id },
            ExecutableValueRefItemLocation {
                module_id,
                item_index,
                owner_node_key: node_key.clone(),
            },
        );
    };
    match &item.kind {
        nia_item_tree::ItemTreeNodeKind::Function(function)
            if !function.is_const && function.body.is_some() =>
        {
            insert(&function.node_key);
        }
        nia_item_tree::ItemTreeNodeKind::Binding(binding)
            if !binding.is_const() && binding.value.is_some() =>
        {
            insert(&binding.node_key);
        }
        nia_item_tree::ItemTreeNodeKind::Trait(item_trait) => {
            for method in &item_trait.methods {
                if method.function.is_const || method.function.body.is_none() {
                    continue;
                }
                insert(&method.function.node_key);
            }
        }
        nia_item_tree::ItemTreeNodeKind::Extend(extend) => {
            for method in &extend.methods {
                if method.function.is_const || method.function.body.is_none() {
                    continue;
                }
                insert(&method.function.node_key);
            }
            for associated_value in &extend.associated_values {
                let binding = &associated_value.binding;
                if binding.is_const() || binding.value.is_none() {
                    continue;
                }
                insert(&binding.node_key);
            }
        }
        _ => {}
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
    fn type_store(&self) -> &nia_ty::TypeStore {
        &self.type_store
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

    fn clear_executable_fact_session(&self) {
        *self
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned") = ExecutableFactSession::default();
    }

    fn retain_executable_facts_after_graph_growth(
        &self,
        body_activated: &HashSet<ModuleId>,
        provider_changes: &HashSet<crate::ProviderDemand>,
    ) {
        self.executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned")
            .retain_after_graph_growth(body_activated, provider_changes);
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
            .map(|module| (module.id, module))
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

    fn executable_checked_modules(&self, set: &ExecutableCheckedModuleSet) -> Vec<CheckedModule> {
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

    fn clear_executable_checked_module_sets(&self) {
        let mut store = self
            .executable_checked_modules
            .write()
            .expect("executable checked module store lock poisoned");
        store.sets.clear();
    }

    fn module_field<T, K>(
        &self,
        db: &QueryDb<CompilerContext>,
        key: &K,
        module_id: ModuleId,
        field: impl FnOnce(&CompilerInputModule) -> T,
    ) -> T
    where
        K: QueryKey<CompilerContext>,
    {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        let Some(module) = inputs.loaded_module(module_id) else {
            db.invalid_input(key, format!("missing loaded module {module_id:?}"));
        };
        field(module)
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

    fn loaded_module(&self, module_id: ModuleId) -> Option<CompilerInputModule> {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        inputs
            .modules_by_id
            .get(&module_id)
            .and_then(|index| inputs.modules.get(*index))
            .cloned()
    }

    fn module_path(&self, db: &QueryDb<CompilerContext>, module_id: ModuleId) -> SourcePath {
        self.module_field(db, &ModulePathQuery(module_id), module_id, |module| {
            module.path.clone()
        })
    }

    fn module_source_version(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> SourceVersion {
        self.module_field(
            db,
            &ModuleSourceVersionQuery(module_id),
            module_id,
            |module| module.source_version,
        )
    }

    fn module_origins(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> NodeOriginTable {
        self.module_field(db, &ModuleOriginsQuery(module_id), module_id, |module| {
            module.origins.clone()
        })
    }

    fn module_parse_errors(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> Vec<ParseError> {
        self.module_field(
            db,
            &ModuleParseErrorsQuery(module_id),
            module_id,
            |module| module.parse_errors.clone(),
        )
    }

    fn module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> Arc<ModuleItemTree> {
        self.module_field(
            db,
            &ModuleItemTreeInputQuery(module_id),
            module_id,
            |module| Arc::clone(&module.item_tree),
        )
    }

    fn declaration_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> Arc<ModuleItemTree> {
        self.module_field(
            db,
            &DeclarationModuleItemTreeInputQuery(module_id),
            module_id,
            |module| Arc::clone(&module.item_tree),
        )
    }

    fn active_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> Arc<ActiveModuleItemTree> {
        self.module_field(
            db,
            &ActiveModuleItemTreeInputQuery(module_id),
            module_id,
            |module| Arc::clone(&module.active_item_tree),
        )
    }

    fn declaration_active_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> Arc<ActiveModuleItemTree> {
        self.module_field(
            db,
            &DeclarationActiveModuleItemTreeInputQuery(module_id),
            module_id,
            |module| Arc::clone(&module.active_item_tree),
        )
    }

    fn signature_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
        set: nia_item_tree::SignatureItemSet,
    ) -> ActiveModuleItemTree {
        self.module_field(
            db,
            &SignatureItemTreeQuery(module_id, set),
            module_id,
            |module| module.active_item_tree.signature_items(set),
        )
    }

    fn signature_const_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> ActiveModuleItemTree {
        self.module_field(
            db,
            &SignatureConstItemTreeQuery(module_id),
            module_id,
            |module| module.active_item_tree.const_signature_items(),
        )
    }

    fn module_provider_summary(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> nia_provider_summary::ProviderSummary {
        self.module_field(
            db,
            &ExtensionProviderSummaryQuery(module_id),
            module_id,
            |module| module.provider_summary.clone(),
        )
    }

    fn path_for_module(&self, module_id: ModuleId) -> SourcePath {
        self.loaded_module(module_id)
            .unwrap_or_else(|| panic!("Nia ICE: missing loaded module {module_id:?}"))
            .path
            .clone()
    }

    fn module_graph(&self) -> ModuleGraph {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .graph
            .clone()
    }

    fn module_graph_node(&self, module_id: ModuleId) -> Option<Arc<ModuleNode>> {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .graph
            .get(module_id)
            .cloned()
            .map(Arc::new)
    }

    fn module_graph_entry(&self) -> ModuleId {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .graph
            .entry()
    }

    fn module_graph_path(&self, module_id: ModuleId) -> Option<nia_imports::ModulePath> {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .graph
            .get(module_id)
            .map(|module| module.module_path.clone())
    }

    fn module_graph_parent(&self, module_id: ModuleId) -> Option<ModuleId> {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .graph
            .get(module_id)
            .and_then(|module| module.parent)
    }

    fn module_graph_child(
        &self,
        module_id: ModuleId,
        name: &SymbolId,
    ) -> Option<(ModuleId, nia_ids::Visibility)> {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        let module = inputs.graph.get(module_id)?;
        let target = module.children.get(name).copied()?;
        let declaration = module
            .declarations
            .iter()
            .find(|declaration| declaration.name == *name && declaration.target == target)?;
        Some((target, declaration.visibility))
    }

    fn module_package_root(&self, package: &SymbolId) -> Option<ModuleId> {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .graph
            .package_root(package)
    }

    fn public_surfaces(&self) -> PublicSurfacesValue {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .public_surfaces
            .clone()
    }

    fn public_surface_module(&self, module_id: ModuleId, name: &SymbolId) -> Option<ModuleId> {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .public_surfaces
            .surfaces
            .get(module_id)?
            .lookup_module(name)
    }

    fn public_surface_value(
        &self,
        module_id: ModuleId,
        name: &SymbolId,
    ) -> Option<nia_defs::PublicItem> {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .public_surfaces
            .surfaces
            .get(module_id)?
            .lookup_value(name)
            .cloned()
    }

    fn public_surface_type(
        &self,
        module_id: ModuleId,
        name: &SymbolId,
    ) -> Option<nia_defs::PublicItem> {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .public_surfaces
            .surfaces
            .get(module_id)?
            .lookup_type(name)
            .cloned()
    }

    fn public_using_scopes(&self) -> PublicUsingScopesValue {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .public_using_scopes
            .clone()
    }

    fn executable_value_ref_item(
        &self,
        owner: GlobalDefId,
    ) -> Option<Arc<ExecutableValueRefItemInput>> {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        let location = inputs.executable_value_ref_items.get(&owner)?;
        let module = inputs.loaded_module(location.module_id)?;
        module.active_item_tree.items.get(location.item_index)?;
        Some(Arc::new(ExecutableValueRefItemInput {
            active_item_tree: module.active_item_tree.clone(),
            item_index: location.item_index,
            owner_node_key: location.owner_node_key.clone(),
        }))
    }

    fn using_scope_module(&self, module_id: ModuleId, name: &SymbolId) -> Option<ModuleId> {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .public_using_scopes
            .using_scopes
            .get(&module_id)?
            .lookup_module(name)
    }

    fn using_scope_value(
        &self,
        module_id: ModuleId,
        name: &SymbolId,
    ) -> Option<nia_defs::UsingEntry> {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .public_using_scopes
            .using_scopes
            .get(&module_id)?
            .lookup_value(name)
            .cloned()
    }

    fn using_scope_type(
        &self,
        module_id: ModuleId,
        name: &SymbolId,
    ) -> Option<nia_defs::UsingEntry> {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .public_using_scopes
            .using_scopes
            .get(&module_id)?
            .lookup_type(name)
            .cloned()
    }

    fn using_scope_unresolved(&self, module_id: ModuleId, name: &SymbolId) -> bool {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .public_using_scopes
            .using_scopes
            .get(&module_id)
            .is_some_and(|scope| scope.has_unresolved_name(name))
    }

    fn load_diagnostics(&self) -> Vec<ProgramDiagnostic> {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .diagnostics
            .clone()
    }

    fn target(&self) -> TargetConfig {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .target
            .clone()
    }

    fn symbols(&self) -> nia_symbol_table::SymbolTable {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .symbols
            .clone()
    }

    fn runtime(&self) -> crate::RuntimeModel {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .runtime
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
    changed_graph_modules: HashSet<ModuleId>,
    changed_graph_paths: HashSet<ModuleId>,
    changed_graph_parents: HashSet<ModuleId>,
    changed_graph_children: HashSet<(ModuleId, SymbolId)>,
    changed_package_roots: HashSet<SymbolId>,
    public_surfaces_changed: bool,
    changed_public_surface_modules: HashSet<ModuleId>,
    changed_public_surface_module_names: HashSet<(ModuleId, SymbolId)>,
    changed_public_surface_value_names: HashSet<(ModuleId, SymbolId)>,
    changed_public_surface_type_names: HashSet<(ModuleId, SymbolId)>,
    public_using_scopes_changed: bool,
    changed_public_using_scope_modules: HashSet<ModuleId>,
    changed_using_scope_module_names: HashSet<(ModuleId, SymbolId)>,
    changed_using_scope_value_names: HashSet<(ModuleId, SymbolId)>,
    changed_using_scope_type_names: HashSet<(ModuleId, SymbolId)>,
    changed_using_scope_unresolved_names: HashSet<(ModuleId, SymbolId)>,
    changed_executable_value_ref_items: HashSet<GlobalDefId>,
    executable_roots_changed: bool,
    body_activated_modules: HashSet<ModuleId>,
    provider_changes: HashSet<crate::ProviderDemand>,
    executable_fact_inputs_changed: bool,
    loaded_modules_changed: bool,
    loaded_diagnostics_changed: bool,
    target_changed: bool,
    runtime_changed: bool,
    optimization_changed: bool,
    changed_modules: Vec<ChangedModuleInput>,
}

impl CompilerInputDiff {
    fn between(old: &CompilerInputs, new: &CompilerInputs) -> Self {
        let changed_modules = changed_loaded_modules(old, new);
        let (
            changed_public_surface_module_names,
            changed_public_surface_value_names,
            changed_public_surface_type_names,
        ) = changed_public_surface_names(old, new);
        let (
            changed_using_scope_module_names,
            changed_using_scope_value_names,
            changed_using_scope_type_names,
            changed_using_scope_unresolved_names,
        ) = changed_using_scope_names(old, new);
        Self {
            graph_changed: old.graph != new.graph,
            graph_entry_changed: old.graph.entry() != new.graph.entry(),
            changed_graph_modules: changed_graph_modules(old, new),
            changed_graph_paths: changed_graph_paths(old, new),
            changed_graph_parents: changed_graph_parents(old, new),
            changed_graph_children: changed_graph_children(old, new),
            changed_package_roots: changed_package_roots(old, new),
            public_surfaces_changed: old.public_surfaces != new.public_surfaces,
            changed_public_surface_modules: changed_public_surface_modules(old, new),
            changed_public_surface_module_names,
            changed_public_surface_value_names,
            changed_public_surface_type_names,
            public_using_scopes_changed: old.public_using_scopes != new.public_using_scopes,
            changed_public_using_scope_modules: changed_public_using_scope_modules(old, new),
            changed_using_scope_module_names,
            changed_using_scope_value_names,
            changed_using_scope_type_names,
            changed_using_scope_unresolved_names,
            changed_executable_value_ref_items: changed_executable_value_ref_items(old, new),
            executable_roots_changed: old.entry_module != new.entry_module
                || old.runtime_root_modules != new.runtime_root_modules,
            body_activated_modules: new
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
                .collect(),
            provider_changes: new.provider_changes.iter().cloned().collect(),
            executable_fact_inputs_changed: executable_fact_inputs_changed(old, new),
            loaded_modules_changed: loaded_module_ids(old) != loaded_module_ids(new)
                || loaded_module_identity_assignments(old)
                    != loaded_module_identity_assignments(new),
            loaded_diagnostics_changed: old.diagnostics != new.diagnostics,
            target_changed: old.target != new.target,
            runtime_changed: old.runtime != new.runtime,
            optimization_changed: old.optimization != new.optimization,
            changed_modules,
        }
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

fn changed_executable_value_ref_items(
    old: &CompilerInputs,
    new: &CompilerInputs,
) -> HashSet<GlobalDefId> {
    old.executable_value_ref_items
        .keys()
        .chain(new.executable_value_ref_items.keys())
        .copied()
        .filter(|owner| executable_value_ref_item_changed(old, new, *owner))
        .collect()
}

fn executable_value_ref_item_changed(
    old: &CompilerInputs,
    new: &CompilerInputs,
    owner: GlobalDefId,
) -> bool {
    executable_value_ref_item_snapshot(old, owner) != executable_value_ref_item_snapshot(new, owner)
}

fn executable_value_ref_item_snapshot(
    inputs: &CompilerInputs,
    owner: GlobalDefId,
) -> Option<(
    &ExecutableValueRefItemLocation,
    &nia_item_tree::ItemTreeNode,
    &HashSet<Span>,
)> {
    let location = inputs.executable_value_ref_items.get(&owner)?;
    let module = inputs.loaded_module(location.module_id)?;
    Some((
        location,
        module.active_item_tree.items.get(location.item_index)?,
        &module.active_item_tree.inactive_spans,
    ))
}

fn changed_graph_modules(old: &CompilerInputs, new: &CompilerInputs) -> HashSet<ModuleId> {
    old.graph
        .modules()
        .map(|module| module.id)
        .chain(new.graph.modules().map(|module| module.id))
        .filter(|module_id| old.graph.get(*module_id) != new.graph.get(*module_id))
        .collect()
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

fn changed_public_surface_modules(old: &CompilerInputs, new: &CompilerInputs) -> HashSet<ModuleId> {
    old.public_surfaces
        .surfaces
        .iter()
        .map(|(module_id, _)| *module_id)
        .chain(
            new.public_surfaces
                .surfaces
                .iter()
                .map(|(module_id, _)| *module_id),
        )
        .filter(|module_id| {
            old.public_surfaces.surfaces.get(*module_id)
                != new.public_surfaces.surfaces.get(*module_id)
        })
        .collect()
}

type ChangedPublicSurfaceNames = (
    HashSet<(ModuleId, SymbolId)>,
    HashSet<(ModuleId, SymbolId)>,
    HashSet<(ModuleId, SymbolId)>,
);

fn changed_public_surface_names(
    old: &CompilerInputs,
    new: &CompilerInputs,
) -> ChangedPublicSurfaceNames {
    let module_ids = old
        .public_surfaces
        .surfaces
        .iter()
        .map(|(module_id, _)| *module_id)
        .chain(
            new.public_surfaces
                .surfaces
                .iter()
                .map(|(module_id, _)| *module_id),
        )
        .collect::<HashSet<_>>();
    let mut changed_modules = HashSet::new();
    let mut changed_values = HashSet::new();
    let mut changed_types = HashSet::new();
    for module_id in module_ids {
        let old_surface = old.public_surfaces.surfaces.get(module_id);
        let new_surface = new.public_surfaces.surfaces.get(module_id);
        let module_names = old_surface
            .into_iter()
            .flat_map(|surface| surface.modules.keys().copied())
            .chain(
                new_surface
                    .into_iter()
                    .flat_map(|surface| surface.modules.keys().copied()),
            )
            .collect::<HashSet<_>>();
        let value_names = old_surface
            .into_iter()
            .flat_map(|surface| surface.values.keys().copied())
            .chain(
                new_surface
                    .into_iter()
                    .flat_map(|surface| surface.values.keys().copied()),
            )
            .collect::<HashSet<_>>();
        let type_names = old_surface
            .into_iter()
            .flat_map(|surface| surface.types.keys().copied())
            .chain(
                new_surface
                    .into_iter()
                    .flat_map(|surface| surface.types.keys().copied()),
            )
            .collect::<HashSet<_>>();
        for name in module_names {
            if old_surface.and_then(|surface| surface.lookup_module(&name))
                != new_surface.and_then(|surface| surface.lookup_module(&name))
            {
                changed_modules.insert((module_id, name));
            }
        }
        for name in value_names {
            if old_surface.and_then(|surface| surface.lookup_value(&name))
                != new_surface.and_then(|surface| surface.lookup_value(&name))
            {
                changed_values.insert((module_id, name));
            }
        }
        for name in type_names {
            if old_surface.and_then(|surface| surface.lookup_type(&name))
                != new_surface.and_then(|surface| surface.lookup_type(&name))
            {
                changed_types.insert((module_id, name));
            }
        }
    }
    (changed_modules, changed_values, changed_types)
}

fn changed_public_using_scope_modules(
    old: &CompilerInputs,
    new: &CompilerInputs,
) -> HashSet<ModuleId> {
    old.public_using_scopes
        .using_scopes
        .keys()
        .chain(new.public_using_scopes.using_scopes.keys())
        .copied()
        .filter(|module_id| {
            old.public_using_scopes.using_scopes.get(module_id)
                != new.public_using_scopes.using_scopes.get(module_id)
        })
        .collect()
}

type ChangedUsingScopeNames = (
    HashSet<(ModuleId, SymbolId)>,
    HashSet<(ModuleId, SymbolId)>,
    HashSet<(ModuleId, SymbolId)>,
    HashSet<(ModuleId, SymbolId)>,
);

fn changed_using_scope_names(old: &CompilerInputs, new: &CompilerInputs) -> ChangedUsingScopeNames {
    let module_ids = old
        .public_using_scopes
        .using_scopes
        .keys()
        .chain(new.public_using_scopes.using_scopes.keys())
        .copied()
        .collect::<HashSet<_>>();
    let mut changed_modules = HashSet::new();
    let mut changed_values = HashSet::new();
    let mut changed_types = HashSet::new();
    let mut changed_unresolved = HashSet::new();
    for module_id in module_ids {
        let old_scope = old.public_using_scopes.using_scopes.get(&module_id);
        let new_scope = new.public_using_scopes.using_scopes.get(&module_id);
        let module_names = old_scope
            .into_iter()
            .flat_map(|scope| scope.modules.keys().copied())
            .chain(
                new_scope
                    .into_iter()
                    .flat_map(|scope| scope.modules.keys().copied()),
            )
            .collect::<HashSet<_>>();
        let value_names = old_scope
            .into_iter()
            .flat_map(|scope| scope.values.keys().copied())
            .chain(
                new_scope
                    .into_iter()
                    .flat_map(|scope| scope.values.keys().copied()),
            )
            .collect::<HashSet<_>>();
        let type_names = old_scope
            .into_iter()
            .flat_map(|scope| scope.types.keys().copied())
            .chain(
                new_scope
                    .into_iter()
                    .flat_map(|scope| scope.types.keys().copied()),
            )
            .collect::<HashSet<_>>();
        let unresolved_names = old_scope
            .into_iter()
            .flat_map(|scope| scope.unresolved_names.iter().copied())
            .chain(
                new_scope
                    .into_iter()
                    .flat_map(|scope| scope.unresolved_names.iter().copied()),
            )
            .collect::<HashSet<_>>();
        for name in module_names {
            if old_scope.and_then(|scope| scope.lookup_module(&name))
                != new_scope.and_then(|scope| scope.lookup_module(&name))
            {
                changed_modules.insert((module_id, name));
            }
        }
        for name in value_names {
            if old_scope.and_then(|scope| scope.lookup_value(&name))
                != new_scope.and_then(|scope| scope.lookup_value(&name))
            {
                changed_values.insert((module_id, name));
            }
        }
        for name in type_names {
            if old_scope.and_then(|scope| scope.lookup_type(&name))
                != new_scope.and_then(|scope| scope.lookup_type(&name))
            {
                changed_types.insert((module_id, name));
            }
        }
        for name in unresolved_names {
            if old_scope.is_some_and(|scope| scope.has_unresolved_name(&name))
                != new_scope.is_some_and(|scope| scope.has_unresolved_name(&name))
            {
                changed_unresolved.insert((module_id, name));
            }
        }
    }
    (
        changed_modules,
        changed_values,
        changed_types,
        changed_unresolved,
    )
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
            (Some(old), Some(new)) if old.id == new.id => Self {
                ids,
                path: old.path != new.path,
                source_identity: old.source_identity != new.source_identity,
                source_version: old.source_version != new.source_version,
                origins: old.origins != new.origins,
                parse_errors: old.parse_errors != new.parse_errors,
                item_tree: !old.item_tree.definition_eq(&new.item_tree),
                declaration_item_tree: !old.item_tree.declaration_eq(&new.item_tree),
                full_item_tree: old.item_tree != new.item_tree,
                active_item_tree: !old.active_item_tree.definition_eq(&new.active_item_tree),
                declaration_active_item_tree: !old
                    .active_item_tree
                    .declaration_eq(&new.active_item_tree),
                signature_function_items: !old
                    .active_item_tree
                    .signature_items(nia_item_tree::SignatureItemSet::Functions)
                    .declaration_eq(
                        &new.active_item_tree
                            .signature_items(nia_item_tree::SignatureItemSet::Functions),
                    ),
                signature_extension_function_items: !old
                    .active_item_tree
                    .signature_items(nia_item_tree::SignatureItemSet::ExtensionFunctions)
                    .declaration_eq(
                        &new.active_item_tree
                            .signature_items(nia_item_tree::SignatureItemSet::ExtensionFunctions),
                    ),
                signature_value_items: !old
                    .active_item_tree
                    .signature_items(nia_item_tree::SignatureItemSet::Values)
                    .declaration_eq(
                        &new.active_item_tree
                            .signature_items(nia_item_tree::SignatureItemSet::Values),
                    ),
                signature_type_items: !old
                    .active_item_tree
                    .signature_items(nia_item_tree::SignatureItemSet::Types)
                    .declaration_eq(
                        &new.active_item_tree
                            .signature_items(nia_item_tree::SignatureItemSet::Types),
                    ),
                provider_summary: old.provider_summary != new.provider_summary,
                signature_trait_items: !old
                    .active_item_tree
                    .signature_items(nia_item_tree::SignatureItemSet::Traits)
                    .declaration_eq(
                        &new.active_item_tree
                            .signature_items(nia_item_tree::SignatureItemSet::Traits),
                    ),
                signature_const_items: old.active_item_tree.const_signature_items()
                    != new.active_item_tree.const_signature_items(),
                full_active_item_tree: old.active_item_tree != new.active_item_tree,
            },
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
    changed.sort_by_key(|module| module.ids.first().copied().unwrap_or(ModuleId(u32::MAX)).0);
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
        module: &'a nia_backend_ir::BackendModule,
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        self_arg: Option<InternedTyId>,
        args: &'a [InternedTyId],
        const_args: &'a [nia_ty::ConstGenericArg],
    }

    fn backend_function_instance_matches_vtable_ref(
        vtable: VtableFunctionInstanceRef<'_>,
        instance_module: &nia_backend_ir::BackendModule,
        instance: &nia_backend_ir::BackendFunctionInstance,
    ) -> bool {
        let VtableFunctionInstanceRef {
            module: vtable_module,
            def_id,
            arg_module_id,
            self_arg,
            args,
            const_args,
        } = vtable;
        if instance.def_id != def_id || instance.arg_module_id != arg_module_id {
            return false;
        }
        let mut target_interner = instance_module.interner.clone();
        let self_arg = match self_arg {
            Some(ty) => {
                let Ok(ty) =
                    nia_ty::try_import_type_into(&mut target_interner, &vtable_module.interner, ty)
                else {
                    return false;
                };
                Some(ty)
            }
            None => None,
        };
        let Ok(args) = args
            .iter()
            .map(|ty| {
                nia_ty::try_import_type_into(&mut target_interner, &vtable_module.interner, *ty)
            })
            .collect::<Result<Vec<_>, _>>()
        else {
            return false;
        };
        let Ok(const_args) = const_args
            .iter()
            .map(|arg| {
                Ok(nia_ty::ConstGenericArg {
                    ty: nia_ty::try_import_type_into(
                        &mut target_interner,
                        &vtable_module.interner,
                        arg.ty,
                    )?,
                    value: arg.value.clone(),
                })
            })
            .collect::<Result<Vec<_>, nia_ty::TypeImportError>>()
        else {
            return false;
        };
        self_arg == instance.self_arg && args == instance.args && const_args == instance.const_args
    }

    fn intern_child(
        graph: &mut ModuleGraph,
        parent: ModuleId,
        child_name: &str,
        visibility: nia_ids::Visibility,
    ) {
        let child = sym(child_name);
        graph
            .intern_declared_child(parent, &child, visibility, Span::default())
            .expect("intern child module");
    }

    fn intern_shallow_child(
        graph: &mut ModuleGraph,
        parent: ModuleId,
        child_name: &str,
        visibility: nia_ids::Visibility,
    ) {
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
            .expect("intern shallow child module");
    }

    fn loaded_program_with_modules(modules: Vec<LoadedModule>) -> LoadedProgram {
        let graph = module_graph_for_loaded_modules(&modules);
        LoadedProgram {
            graph,
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::Bare,
            modules,
            diagnostics: Vec::new(),
        }
    }

    fn module_graph_for_loaded_modules(modules: &[LoadedModule]) -> ModuleGraph {
        let entry = modules
            .first()
            .map(|module| module.path.clone())
            .unwrap_or_else(|| SourcePath::new("main.nia"));
        let mut graph = ModuleGraph::with_symbol_text(entry, Arc::new(test_symbols()));
        let max_id = modules
            .iter()
            .map(|module| module.id.0)
            .max()
            .unwrap_or(graph.entry().0);
        for id in 1..=max_id {
            let entry = graph.entry();
            intern_child(
                &mut graph,
                entry,
                &format!("module{id}"),
                nia_ids::Visibility::Public,
            );
        }
        graph
    }

    fn loaded_program_with_entry_child(
        entry: LoadedModule,
        child_name: &str,
        child: LoadedModule,
    ) -> LoadedProgram {
        let mut graph = ModuleGraph::with_symbol_text(entry.path.clone(), Arc::new(test_symbols()));
        intern_child(
            &mut graph,
            entry.id,
            child_name,
            nia_ids::Visibility::Public,
        );
        LoadedProgram {
            graph,
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::Bare,
            modules: vec![entry, child],
            diagnostics: Vec::new(),
        }
    }

    fn loaded_program_with_shallow_entry_child(
        entry: LoadedModule,
        child_name: &str,
        child: LoadedModule,
    ) -> LoadedProgram {
        let mut graph = ModuleGraph::with_symbol_text(entry.path.clone(), Arc::new(test_symbols()));
        intern_shallow_child(
            &mut graph,
            entry.id,
            child_name,
            nia_ids::Visibility::Public,
        );
        LoadedProgram {
            graph,
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::Bare,
            modules: vec![entry, child],
            diagnostics: Vec::new(),
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
        let source_version = nia_source::SourceVersion {
            id: SourceId(id.0),
            revision,
        };
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
        let inputs = Arc::new(RwLock::new(CompilerInputs::new(CompileRequest::new(
            loaded,
        ))));
        QueryDb::new(CompilerContext {
            inputs,
            providers: CompilerQueryProviders::default(),
            executable_checked_modules: Arc::new(RwLock::new(
                ExecutableCheckedModuleStore::default(),
            )),
            executable_fact_session: Arc::new(std::sync::Mutex::new(
                ExecutableFactSession::default(),
            )),
            type_store: Arc::new(nia_ty::TypeStore::new()),
        })
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
    fn public_options_flow_through_compiler_query_context() {
        for level in [
            NiaOptimizationLevel::O0,
            NiaOptimizationLevel::O1,
            NiaOptimizationLevel::O2,
            NiaOptimizationLevel::O3,
            NiaOptimizationLevel::Os,
            NiaOptimizationLevel::Oz,
        ] {
            let loaded = loaded_program_with_modules(vec![loaded_module(
                ModuleId(0),
                "main.nia",
                r#"
static zeroes: [4]i32 = [0; 4];

fn main() i32 {
    zeroes[0]
}
"#,
            )]);
            let checked =
                CompilerDatabase::new(CompileRequest::new(loaded).with_optimization(level))
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
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }"),
            ])));

        let checked = database.check_program();
        let trace = database.query_trace();

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "checked_program" && dependency.to.name == "checked_module_ids"
        }));
    }

    #[test]
    fn semantic_module_ids_exclude_shallow_facade_modules() {
        let entry = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
pub module facade;

fn main() i32 {
    0
}
"#,
        );
        let facade = loaded_module(
            ModuleId(1),
            "facade.nia",
            r#"
pub fn expensive_or_invalid() i32 {
    missing_symbol
}
"#,
        );
        let db = query_db(loaded_program_with_shallow_entry_child(
            entry, "facade", facade,
        ));

        assert_eq!(
            db.query(ParseOkModuleIdsQuery),
            vec![ModuleId(0), ModuleId(1)]
        );
        assert_eq!(db.query(SemanticModuleIdsQuery), vec![ModuleId(0)]);

        assert_eq!(db.query(CheckedModuleIdsQuery), vec![ModuleId(0)]);
    }

    #[test]
    fn compiler_inputs_index_modules_by_source_identity() {
        let loaded = loaded_program_with_modules(vec![
            loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }"),
            loaded_module(ModuleId(1), "pkg/root.nia", "pub fn value() i32 { 1 }"),
        ]);
        let db = query_db(loaded);

        assert_eq!(
            module_id_for_source_identity(&db, &SourcePath::new("pkg/root.nia").identity()),
            Some(ModuleId(1))
        );
    }

    #[test]
    #[should_panic(expected = "Nia ICE: duplicate loaded module id")]
    fn compiler_inputs_reject_duplicate_module_ids() {
        let _ = CompilerInputs::new(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }"),
            loaded_module(ModuleId(0), "other.nia", "pub fn value() i32 { 1 }"),
        ])));
    }

    #[test]
    #[should_panic(expected = "Nia ICE: duplicate source identity")]
    fn compiler_inputs_reject_duplicate_source_identities() {
        let _ = CompilerInputs::new(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }"),
            loaded_module(ModuleId(1), "main.nia", "pub fn value() i32 { 1 }"),
        ])));
    }

    #[test]
    #[should_panic(expected = "Nia ICE: loaded module")]
    fn compiler_inputs_reject_path_identity_mismatch() {
        let mut module = loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }");
        module.source_identity = SourcePath::new("other.nia").identity();

        let _ = CompilerInputs::new(CompileRequest::new(loaded_program_with_modules(vec![
            module,
        ])));
    }

    #[test]
    fn loaded_module_reorder_invalidates_list_without_field_changes() {
        let old = CompilerInputs::new(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }"),
            loaded_module(ModuleId(1), "pkg/root.nia", "pub fn value() i32 { 1 }"),
        ])));
        let new = CompilerInputs::new(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(ModuleId(1), "pkg/root.nia", "pub fn value() i32 { 1 }"),
            loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }"),
        ])));

        let diff = CompilerInputDiff::between(&old, &new);

        assert!(diff.loaded_modules_changed);
        assert_eq!(diff.changed_modules, Vec::new());
    }

    #[test]
    fn additive_module_growth_preserves_existing_executable_fact_inputs() {
        let entry = loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }");
        let old = CompilerInputs::new(CompileRequest::new(loaded_program_with_modules(vec![
            entry.clone(),
        ])));
        let new = CompilerInputs::new(CompileRequest::new(loaded_program_with_entry_child(
            entry,
            "provider",
            loaded_module(ModuleId(1), "main/provider.nia", "pub fn value() i32 { 1 }"),
        )));

        let diff = CompilerInputDiff::between(&old, &new);

        assert!(diff.loaded_modules_changed);
        assert!(!diff.executable_fact_inputs_changed);
    }

    #[test]
    fn stable_source_identity_with_new_module_id_invalidates_old_key_and_recomputes_new_key() {
        let old = CompilerInputs::new(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(
                ModuleId(0),
                "main.nia",
                "pub struct S { value: i32 } fn main() i32 { 0 }",
            ),
        ])));
        let new = CompilerInputs::new(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(
                ModuleId(7),
                "main.nia",
                "pub struct S { value: i32 } fn main() i32 { 0 }",
            ),
        ])));
        let diff = CompilerInputDiff::between(&old, &new);

        assert!(diff.loaded_modules_changed);
        assert_eq!(diff.changed_modules.len(), 1);
        assert_eq!(diff.changed_modules[0].ids, vec![ModuleId(0), ModuleId(7)]);

        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(
                    ModuleId(0),
                    "main.nia",
                    "pub struct S { value: i32 } fn main() i32 { 0 }",
                ),
            ])));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(
                ModuleId(7),
                "main.nia",
                "pub struct S { value: i32 } fn main() i32 { 0 }",
            ),
        ])));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.description.as_str())
            .collect::<Vec<_>>();

        assert!(
            invalidated.contains(&"module_path(ModuleId(0))"),
            "{invalidated:?}"
        );
        assert!(
            invalidated.contains(&"checked_module::CheckedModuleQuery(ModuleId(0))"),
            "{invalidated:?}"
        );
        assert!(
            invalidated.contains(&"loaded_modules::LoadedModulesQuery"),
            "{invalidated:?}"
        );

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        assert_eq!(second.modules[0].id, ModuleId(7));
    }

    #[test]
    fn same_module_id_with_new_source_identity_is_replacement() {
        let old = CompilerInputs::new(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }"),
        ])));
        let new = CompilerInputs::new(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(ModuleId(0), "other.nia", "fn main() i32 { 0 }"),
        ])));

        let diff = CompilerInputDiff::between(&old, &new);

        assert!(diff.loaded_modules_changed);
        assert_eq!(diff.changed_modules.len(), 2);
        assert!(diff.changed_modules.iter().all(|module| {
            module.ids == vec![ModuleId(0)]
                && module.path
                && module.source_identity
                && module.source_version
                && module.item_tree
                && module.full_item_tree
        }));
    }

    #[test]
    fn compiler_database_update_invalidates_changed_module_field_inputs() {
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }"),
            ])));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module_with_revision(
                ModuleId(0),
                "main.nia",
                "fn main() i32 { true }",
                SourceRevision(1),
            ),
        ])));
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
            !invalidated.contains(&"module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(
            invalidated.contains(&"full_module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"checked_program"), "{invalidated:?}");

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
    fn timing_mode_update_does_not_invalidate_semantic_queries() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "fn main() i32 { 0 }",
        )]);
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
        let loaded = loaded_program_with_shallow_entry_child(
            loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }"),
            "provider",
            loaded_module(ModuleId(1), "main/provider.nia", "pub fn value() i32 { 1 }"),
        );
        let database = CompilerDatabase::new(CompileRequest::new(loaded.clone()));
        assert_eq!(
            database.db.query(ExecutableRootModulesQuery),
            (ModuleId(0), Vec::new())
        );
        let _ = database.db.query(TypeResolutionQuery(ModuleId(0)));

        let mut grown = loaded;
        assert!(grown.graph.mark_semantic_selected(ModuleId(1)));
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
            database.db.query(ExecutableRootModulesQuery),
            (ModuleId(0), Vec::new())
        );
    }

    #[test]
    fn additive_provider_graph_growth_reuses_existing_executable_facts() {
        let entry = loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }");
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                entry.clone(),
            ])));

        let _ = database.executable_provider_demands();
        {
            let session = database
                .db
                .context()
                .executable_fact_session
                .lock()
                .expect("executable fact session lock poisoned");
            assert!(session.modules.contains_key(&ModuleId(0)));
            assert!(
                session
                    .caches
                    .body_resolution_inputs
                    .borrow()
                    .contains_key(&ModuleId(0))
            );
        }

        database.update(CompileRequest::new(loaded_program_with_entry_child(
            entry,
            "provider",
            loaded_module(ModuleId(1), "main/provider.nia", "pub fn value() i32 { 1 }"),
        )));
        let session = database
            .db
            .context()
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned");
        assert!(session.modules.contains_key(&ModuleId(0)));
        assert!(
            session
                .caches
                .body_resolution_inputs
                .borrow()
                .contains_key(&ModuleId(0))
        );
    }

    #[test]
    fn provider_changes_discard_affected_executable_fact_caches() {
        let entry = loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }");
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                entry.clone(),
            ])));
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
                .get_mut(&ModuleId(0))
                .expect("entry executable facts");
            state
                .unowned_provider_demands
                .insert(provider_changes[0].clone());
            state.provider_demands.insert(provider_changes[0].clone());
        }

        database.update(
            CompileRequest::new(loaded_program_with_entry_child(
                entry,
                "provider",
                loaded_module(ModuleId(1), "main/provider.nia", "pub fn value() i32 { 1 }"),
            ))
            .with_provider_changes(provider_changes),
        );

        let session = database
            .db
            .context()
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned");
        assert!(!session.modules.contains_key(&ModuleId(0)));
        assert!(
            !session
                .caches
                .body_resolution_inputs
                .borrow()
                .contains_key(&ModuleId(0))
        );
    }

    #[test]
    fn semantic_provider_activation_preserves_resolved_caller_facts() {
        let entry = loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }");
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                entry.clone(),
            ])));
        let _ = database.executable_provider_demands();
        let provider_change = crate::ProviderDemand {
            source_path: SourcePath::new("main.nia"),
            request: crate::ProviderRequest::ModuleSemantic {
                module_id: ModuleId(1),
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
                .get_mut(&ModuleId(0))
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
            CompileRequest::new(loaded_program_with_entry_child(
                entry,
                "provider",
                loaded_module(ModuleId(1), "main/provider.nia", "pub fn value() i32 { 1 }"),
            ))
            .with_provider_changes([provider_change]),
        );

        let session = database
            .db
            .context()
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned");
        let state = session
            .modules
            .get(&ModuleId(0))
            .expect("preserved entry executable facts");
        assert!(state.checked_functions.contains(&checked_function));
        assert!(
            session
                .caches
                .body_resolution_inputs
                .borrow()
                .contains_key(&ModuleId(0))
        );
    }

    #[test]
    fn method_provider_change_removes_only_affected_function_diagnostics() {
        let entry = loaded_module(
            ModuleId(0),
            "main.nia",
            "struct Value {} fn helper() i32 { 1 } fn main(value: Value) i32 { value.missing() }",
        );
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                entry.clone(),
            ])));
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
            let state = session.modules.get(&ModuleId(0)).expect("entry facts");
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

        database.update(
            CompileRequest::new(loaded_program_with_entry_child(
                entry,
                "provider",
                loaded_module(ModuleId(1), "main/provider.nia", "pub fn value() i32 { 1 }"),
            ))
            .with_provider_changes(provider_changes),
        );

        let session = database
            .db
            .context()
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned");
        let state = session
            .modules
            .get(&ModuleId(0))
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
                .contains_key(&ModuleId(0))
        );
    }

    #[test]
    fn revision_only_update_keeps_declaration_and_type_queries_cached() {
        let source = "pub struct S { value: i32 } fn main() i32 { let value: i32 = 0; value }";
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module_with_revision(ModuleId(0), "main.nia", source, SourceRevision(0)),
            ])));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module_with_revision(ModuleId(0), "main.nia", source, SourceRevision(1)),
        ])));
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
            !invalidated.contains(&"module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(
            !invalidated.contains(&"declaration_type_lowering"),
            "{invalidated:?}"
        );
        assert!(!invalidated.contains(&"item_signatures"), "{invalidated:?}");
        assert!(
            !invalidated.iter().any(|name| is_body_signature_query(name)),
            "{invalidated:?}"
        );
        let before_second_check = database.query_trace();

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        let after_second_check = database.query_trace();

        assert_eq!(
            query_executions(&before_second_check, "declaration_type_lowering"),
            query_executions(&after_second_check, "declaration_type_lowering"),
        );
        assert_eq!(
            query_executions(&before_second_check, "item_signatures"),
            query_executions(&after_second_check, "item_signatures"),
        );
        assert!(
            query_cache_hits(&after_second_check, "item_signatures")
                > query_cache_hits(&before_second_check, "item_signatures"),
        );
    }

    #[test]
    fn type_store_preserves_published_slots_across_database_updates() {
        let module_id = ModuleId(0);
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module_with_revision(
                    module_id,
                    "main.nia",
                    "pub struct S { value: i32 }",
                    SourceRevision(0),
                ),
            ])));
        let first = database.db.query(TypeLoweringQuery(module_id));
        let first_i32 = first.interner.primitive(nia_ty::PrimitiveTy::I32);

        database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module_with_revision(
                module_id,
                "main.nia",
                "pub struct S { value: i32, flag: &bool }",
                SourceRevision(1),
            ),
        ])));
        let second = database.db.query(TypeLoweringQuery(module_id));

        assert_eq!(first.interner.interner_id(), second.interner.interner_id());
        assert!(first.interner.is_prefix_of(&second.interner));
        assert_eq!(
            second.interner.get(first_i32),
            Some(&nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I32))
        );
        assert!(second.interner.len() > first.interner.len());
    }

    #[test]
    fn type_normalization_appends_to_the_session_type_store() {
        let module_id = ModuleId(0);
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(
                    module_id,
                    "main.nia",
                    "type ByteRef = &u8; pub fn read(value: ByteRef) u8 { 0 }",
                ),
            ])));
        let lowering = database.db.query(TypeLoweringQuery(module_id));
        let normalization = database.db.query(TypeNormalizationQuery(module_id));
        let stored = database.db.context().type_store.module_snapshot(module_id);

        assert_eq!(
            lowering.interner.interner_id(),
            normalization.interner.interner_id()
        );
        assert!(lowering.interner.is_prefix_of(&normalization.interner));
        assert!(normalization.interner.is_prefix_of(&stored));
        for (ty_id, kind) in lowering.interner.iter() {
            assert_eq!(normalization.interner.get(ty_id), Some(kind));
            assert_eq!(stored.get(ty_id), Some(kind));
        }
        assert!(
            normalization
                .normalized
                .iter()
                .any(|(source, normalized)| source != normalized)
        );
    }

    #[test]
    fn const_phases_append_to_one_session_type_store_shard() {
        let module_id = ModuleId(0);
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(
                    module_id,
                    "main.nia",
                    r#"
const values = 0usize..3usize;
const width: usize = values.end();

fn main() i32 { 0 }
"#,
                ),
            ])));
        let normalization = database.db.query(TypeNormalizationQuery(module_id));
        let before_const = database.db.context().type_store.module_snapshot(module_id);

        let _ = database.db.query(ConstArrayLengthsQuery(module_id));
        let after_array_lengths = database.db.context().type_store.module_snapshot(module_id);
        let _ = database.db.query(ConstEnumValuesQuery(module_id));
        let after_enum_values = database.db.context().type_store.module_snapshot(module_id);
        let _ = database.db.query(ConstValuesQuery(module_id));
        let after_values = database.db.context().type_store.module_snapshot(module_id);
        let _ = database.db.query(ConstTypedFactsQuery(module_id));
        let after_typed_facts = database.db.context().type_store.module_snapshot(module_id);
        let _ = database.db.query(ConstQuery(module_id));
        let after_check = database.db.context().type_store.module_snapshot(module_id);

        assert!(normalization.interner.is_prefix_of(&before_const));
        assert!(before_const.is_prefix_of(&after_array_lengths));
        assert!(after_array_lengths.is_prefix_of(&after_enum_values));
        assert!(after_enum_values.is_prefix_of(&after_values));
        assert!(after_values.is_prefix_of(&after_typed_facts));
        assert_eq!(after_typed_facts, after_check);
        assert!(after_check.len() > before_const.len());
    }

    #[test]
    fn body_check_appends_to_the_session_type_store_shard() {
        let module_id = ModuleId(0);
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(
                    module_id,
                    "main.nia",
                    r#"
fn main() i32 {
    let values = [1i32, 2i32, 3i32];
    values[0]
}
"#,
                ),
            ])));
        let _ = database.db.query(ConstQuery(module_id));
        let before_body = database.db.context().type_store.module_snapshot(module_id);

        let checked = database.db.query(BodyCheckQuery(module_id));
        let after_body = database.db.context().type_store.module_snapshot(module_id);

        assert!(before_body.is_prefix_of(&after_body));
        assert_eq!(checked.ir.interner, after_body);
        assert!(after_body.iter().any(|(ty, kind)| {
            before_body.get(ty).is_none()
                && matches!(
                    kind,
                    nia_ty::TyKind::Array {
                        len: nia_ty::ArrayLenTy::ConstValue(3),
                        ..
                    }
                )
        }));
    }

    #[test]
    fn signature_and_full_normalization_share_ids_in_either_query_order() {
        fn assert_order(signature_first: bool) {
            let module_id = ModuleId(0);
            let database =
                CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                    loaded_module(
                        module_id,
                        "main.nia",
                        "type Ref[T] = &T; pub fn read(value: Ref[u16]) u16 { 0 }",
                    ),
                ])));
            let signature_key = SignatureTypeNormalizationQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Functions,
            );
            let (signature, full) = if signature_first {
                let signature = database.db.query(signature_key);
                let full = database.db.query(TypeNormalizationQuery(module_id));
                (signature, full)
            } else {
                let full = database.db.query(TypeNormalizationQuery(module_id));
                let signature = database.db.query(signature_key);
                (signature, full)
            };

            assert_eq!(
                signature.interner.interner_id(),
                full.interner.interner_id()
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
        let loaded = || {
            loaded_program_with_modules(vec![loaded_module(
                ModuleId(0),
                "main.nia",
                "pub struct S { value: i32 }",
            )])
        };
        let first = CompilerDatabase::new(CompileRequest::new(loaded()));
        let second = CompilerDatabase::new(CompileRequest::new(loaded()));
        let first_types = first.db.query(TypeLoweringQuery(ModuleId(0)));
        let second_types = second.db.query(TypeLoweringQuery(ModuleId(0)));
        let first_i32 = first_types.interner.primitive(nia_ty::PrimitiveTy::I32);
        let second_i32 = second_types.interner.primitive(nia_ty::PrimitiveTy::I32);

        assert_ne!(
            first_types.interner.interner_id().store_id(),
            second_types.interner.interner_id().store_id()
        );
        assert_ne!(first_i32, second_i32);
        assert_eq!(first_types.interner.get(second_i32), None);
        assert_eq!(second_types.interner.get(first_i32), None);
    }

    #[test]
    fn function_body_update_keeps_public_surface_queries_cached() {
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(
                    ModuleId(0),
                    "main.nia",
                    "pub struct S { value: i32 } fn main() i32 { 0 }",
                ),
            ])));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module_with_revision(
                ModuleId(0),
                "main.nia",
                "pub struct S { value: i32 } fn main() i32 { 1 }",
                SourceRevision(1),
            ),
        ])));
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
            !invalidated.contains(&"module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"body_check"), "{invalidated:?}");
        assert!(!invalidated.contains(&"loaded_modules"), "{invalidated:?}");
        assert!(!invalidated.contains(&"public_surfaces"), "{invalidated:?}");
        assert!(
            !invalidated.contains(&"public_using_scopes"),
            "{invalidated:?}"
        );

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
    }

    #[test]
    fn function_body_type_update_keeps_signature_queries_cached() {
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(
                    ModuleId(0),
                    "main.nia",
                    "pub struct S { value: i32 } fn main() i32 { let value: i32 = 0; value }",
                ),
            ])));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module_with_revision(
                ModuleId(0),
                "main.nia",
                "pub struct S { value: i32 } fn main() i32 { let value: u8 = 0; value as i32 }",
                SourceRevision(1),
            ),
        ])));
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
            !invalidated.contains(&"declaration_type_lowering"),
            "{invalidated:?}"
        );
        assert!(!invalidated.contains(&"item_signatures"), "{invalidated:?}");
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
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(
                    ModuleId(0),
                    "main.nia",
                    "pub struct S { value: i32 } fn main() i32 { let value: i32 = 0; value }",
                ),
            ])));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module_with_revision(
                ModuleId(0),
                "main.nia",
                "pub struct S { value: i32 } fn main() i32 { let value: u8 = 0; value as i32 }",
                SourceRevision(1),
            ),
        ])));
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
    fn function_signature_update_keeps_definition_queries_cached() {
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(
                    ModuleId(0),
                    "main.nia",
                    "pub struct S { value: i32 } fn helper() i32 { 1 } fn main() i32 { helper() }",
                ),
            ])));

        let first = database.codegen_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module_with_revision(
                ModuleId(0),
                "main.nia",
                "pub struct S { value: i32 } fn helper() u8 { 1 } fn main() i32 { helper() as i32 }",
                SourceRevision(1),
            ),
        ])));
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
            !invalidated.contains(&"module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(!invalidated.contains(&"module_defs"), "{invalidated:?}");
        assert!(!invalidated.contains(&"public_surfaces"), "{invalidated:?}");
        assert!(
            !invalidated.contains(&"public_using_scopes"),
            "{invalidated:?}"
        );
    }

    #[test]
    fn function_body_type_update_keeps_signature_program_type_context_cached() {
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(
                    ModuleId(0),
                    "main.nia",
                    "fn main() i32 { let value: i32 = 0; value }",
                ),
                loaded_module(ModuleId(1), "helper.nia", "fn helper() i32 { 1 }"),
            ])));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module_with_revision(
                ModuleId(0),
                "main.nia",
                "fn main() i32 { let value: u8 = 0; value as i32 }",
                SourceRevision(1),
            ),
            loaded_module(ModuleId(1), "helper.nia", "fn helper() i32 { 1 }"),
        ])));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(invalidated.contains(&"type_lowering"), "{invalidated:?}");
        assert!(
            !invalidated.contains(&"declaration_type_lowering"),
            "{invalidated:?}"
        );
        let before_second_check = database.query_trace();

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        let after_second_check = database.query_trace();

        assert_eq!(
            query_executions(&before_second_check, "signature_type_normalization"),
            query_executions(&after_second_check, "signature_type_normalization"),
        );
    }

    #[test]
    fn source_identity_update_invalidates_module_dependent_queries() {
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(
                    ModuleId(0),
                    "main.nia",
                    "pub struct S { value: i32 } fn main() i32 { 0 }",
                ),
            ])));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(
                ModuleId(0),
                "renamed.nia",
                "pub struct S { value: i32 } fn main() i32 { 0 }",
            ),
        ])));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(invalidated.contains(&"loaded_modules"), "{invalidated:?}");
        assert!(invalidated.contains(&"checked_module"), "{invalidated:?}");
        assert!(!invalidated.contains(&"public_surfaces"), "{invalidated:?}");
        assert!(
            !invalidated.contains(&"public_using_scopes"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"module_path"), "{invalidated:?}");

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        assert_eq!(second.modules[0].path.as_str(), "renamed.nia");
    }

    #[test]
    fn source_identity_change_invalidates_loaded_module_list() {
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }"),
            ])));
        let _ = database.check_program();

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(ModuleId(0), "other.nia", "fn main() i32 { 0 }"),
        ])));
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
        fn no_parse_ok_modules(_: &QueryDb<CompilerContext>) -> Vec<ModuleId> {
            Vec::new()
        }

        let providers = CompilerQueryProviders {
            parse_ok_module_ids: no_parse_ok_modules,
            ..CompilerQueryProviders::default()
        };
        let checked = compiler_database_with_providers(
            CompileRequest::new(loaded_program_with_modules(vec![loaded_module(
                ModuleId(0),
                "main.nia",
                "fn main() i32 { 0 }",
            )])),
            providers,
        )
        .codegen_program();

        assert!(checked.modules.is_empty());
    }

    #[test]
    fn missing_loaded_module_id_becomes_query_diagnostic() {
        fn unknown_semantic_module(_: &QueryDb<CompilerContext>) -> Vec<ModuleId> {
            vec![ModuleId(99)]
        }

        let providers = CompilerQueryProviders {
            semantic_module_ids: unknown_semantic_module,
            ..CompilerQueryProviders::default()
        };
        let policy = NiaOptimizationLevel::Oz.policy();
        let checked = compiler_database_with_providers(
            CompileRequest::new(loaded_program_with_modules(vec![loaded_module(
                ModuleId(0),
                "main.nia",
                "fn main() i32 { 0 }",
            )]))
            .with_optimization(NiaOptimizationLevel::Oz),
            providers,
        )
        .codegen_program();

        assert!(checked.modules.is_empty());
        assert_eq!(checked.optimization, policy);
        assert_eq!(checked.backend_lowering.optimization, policy);
        assert_eq!(checked.diagnostics.len(), 1);
        assert!(
            checked.diagnostics[0]
                .diagnostic
                .summary
                .contains("missing loaded module ModuleId(99)")
        );
    }

    #[test]
    fn body_check_resolves_program_signatures_through_precise_signature_queries() {
        let loaded = loaded_program_with_modules(vec![
            loaded_module(
                ModuleId(0),
                "main.nia",
                "using helper::{Alias, value}; fn main() Alias { value() }",
            ),
            loaded_module(
                ModuleId(1),
                "helper.nia",
                "pub type Alias = i32; pub fn value() Alias { 1 }",
            ),
        ]);
        let db = query_db(loaded);

        let checked = db.query(BodyCheckQuery(ModuleId(0)));
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let trace = db.query_trace();

        assert!(trace_has_dependency(
            &trace,
            "body_check",
            "signature_item_signatures"
        ));
        assert!(trace_has_dependency(
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
        let loaded = loaded_program_with_modules(vec![
            loaded_module(
                ModuleId(0),
                "main.nia",
                r#"
module traits;
using entry::traits::{Ops, Value};

fn main() i32 {
    let value = Value {};
    value.used()
}
"#,
            ),
            loaded_module(
                ModuleId(1),
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
            ),
        ]);
        let db = query_db(loaded);

        let checked = db.query(BodyCheckQuery(ModuleId(0)));
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
        let loaded = loaded_program_with_modules(vec![
            loaded_module(
                ModuleId(0),
                "main.nia",
                "module module1; module module2; module module3; module module4; module module5; module module6;",
            ),
            loaded_module(ModuleId(1), "module1.nia", "struct S { value: i32 }"),
            loaded_module(ModuleId(2), "module2.nia", "fn helper() i32 { 1 }"),
            loaded_module(ModuleId(3), "module3.nia", "const WIDTH: usize = 4usize;"),
            loaded_module(
                ModuleId(4),
                "module4.nia",
                "trait Read { fn read(self) i32; }",
            ),
            loaded_module(
                ModuleId(5),
                "module5.nia",
                "struct T {} extend T { pub fn make() T { {} } }",
            ),
            loaded_module(
                ModuleId(6),
                "module6.nia",
                "struct U {} extend U { const WIDTH: usize = 4usize; }",
            ),
        ]);
        let db = query_db(loaded);

        assert_eq!(
            db.query(ProgramSignatureModuleIdsQuery(
                nia_item_tree::SignatureItemSet::Functions
            )),
            vec![ModuleId(2), ModuleId(4), ModuleId(5)]
        );
        assert_eq!(
            db.query(ProgramSignatureModuleIdsQuery(
                nia_item_tree::SignatureItemSet::Values
            )),
            vec![ModuleId(3), ModuleId(6)]
        );
        assert_eq!(
            db.query(ProgramSignatureModuleIdsQuery(
                nia_item_tree::SignatureItemSet::Types
            )),
            vec![ModuleId(1), ModuleId(5), ModuleId(6)]
        );
        assert_eq!(
            db.query(ProgramSignatureModuleIdsQuery(
                nia_item_tree::SignatureItemSet::Traits
            )),
            vec![ModuleId(4), ModuleId(5), ModuleId(6)]
        );
        assert_eq!(
            db.query(ProgramSignatureModuleIdsQuery(
                nia_item_tree::SignatureItemSet::ExtensionFunctions
            )),
            vec![ModuleId(4), ModuleId(5)]
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
        let loaded = loaded_program_with_modules(vec![
            loaded_module(
                ModuleId(0),
                "main.nia",
                "module module1; module module2; module module3; module module4; module module5;",
            ),
            loaded_module(ModuleId(1), "module1.nia", "struct S { value: i32 }"),
            loaded_module(ModuleId(2), "module2.nia", "fn helper() i32 { 1 }"),
            loaded_module(ModuleId(3), "module3.nia", "const WIDTH: usize = 4usize;"),
            loaded_module(
                ModuleId(4),
                "module4.nia",
                "trait Read { fn read(self) i32; }",
            ),
            loaded_module(
                ModuleId(5),
                "module5.nia",
                "struct T {} extend T { pub fn make() T { {} } }",
            ),
        ]);
        let db = query_db(loaded);

        assert_eq!(db.query(ExtensionProviderModuleIdsQuery), vec![ModuleId(5)]);
        let trace = db.query_trace();
        assert!(trace_has_dependency(
            &trace,
            "extension_provider_module_ids",
            "extension_provider_discovery_index"
        ));
        assert!(trace_has_dependency(
            &trace,
            "extension_provider_discovery_index",
            "parse_ok_module_ids"
        ));
        assert!(!trace_has_dependency(
            &trace,
            "extension_provider_discovery_index",
            "semantic_module_ids"
        ));
        assert!(trace_has_dependency(
            &trace,
            "extension_provider_discovery_index",
            "extension_provider_module_eligibility"
        ));
    }

    #[test]
    fn program_type_alias_signature_uses_precise_module_facts() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "struct S { value: i32 } type Alias = S; fn helper() i32 { 1 }",
        )]);
        let db = query_db(loaded);

        let defs = db.query(ModuleDefsQuery(ModuleId(0)));
        let alias_id = defs.module_scope.types.get(&sym("Alias")).unwrap();
        let _ = db.query(ProgramTypeAliasSignatureQuery(GlobalDefId {
            module_id: ModuleId(0),
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
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "struct S { value: i32 } fn helper() i32 { 1 }",
        )]);
        let db = query_db(loaded);

        let layouts = db.query(LayoutsQuery(ModuleId(0)));
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
        let loaded = loaded_program_with_modules(vec![
            loaded_module(
                ModuleId(0),
                "main.nia",
                "module module1; using self::module1::S; struct Holder { value: S }",
            ),
            loaded_module(
                ModuleId(1),
                "module1.nia",
                "pub struct S { value: i32 } fn helper() i32 { 1 }",
            ),
        ]);
        let db = query_db(loaded);

        let layouts = db.query(LayoutsQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "layouts"
                && dependency.from.description.contains("ModuleId(0)")
                && dependency.to.name == "signature_layouts"
                && dependency.to.description.contains("ModuleId(1)")
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "layouts"
                && dependency.from.description.contains("ModuleId(0)")
                && dependency.to.name == "layouts"
                && dependency.to.description.contains("ModuleId(1)")
        }));
    }

    #[test]
    fn abi_check_uses_abi_signature_index_not_body_signatures() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "extern struct S { value: i32 } extern fn take(value: S) void;",
        )]);
        let db = query_db(loaded);

        let _ = db.query(AbiCheckQuery(ModuleId(0)));
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
    fn public_surface_snapshots_are_independent_query_inputs() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "pub struct S { value: i32 }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(PublicSurfacesQuery);
        let _ = db.query(ModulePublicSurfaceQuery(ModuleId(0)));
        let _ = db.query(ModuleUsingScopeQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "public_surfaces" && dependency.to.name == "module_defs"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "public_using_scopes" && dependency.to.name == "public_surfaces"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "module_using_scope"
                && dependency.to.name == "public_using_scopes"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "module_public_surface"
                && dependency.to.name == "public_surfaces"
        }));
    }

    #[test]
    fn executable_value_refs_resolve_only_the_requested_body_item() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "fn helper() i32 { 1 } fn main() i32 { helper() }",
        )]);
        let db = query_db(loaded);
        let defs = db.query(ModuleDefsQuery(ModuleId(0)));
        let main = GlobalDefId {
            module_id: ModuleId(0),
            def_id: defs.module_scope.values.get(&sym("main")).unwrap(),
        };
        let helper = GlobalDefId {
            module_id: ModuleId(0),
            def_id: defs.module_scope.values.get(&sym("helper")).unwrap(),
        };

        let edges = db.query(ExecutableValueRefEdgesQuery(main));
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
        assert!(!trace_has_dependency(
            &trace,
            "executable_value_ref_edges",
            "full_active_module_item_tree"
        ));
    }

    #[test]
    fn executable_value_refs_include_unqualified_static_uses() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "static mut calls: i32 = 0; fn main() i32 { calls += 1; calls }",
        )]);
        let db = query_db(loaded);
        let defs = db.query(ModuleDefsQuery(ModuleId(0)));
        let main = GlobalDefId {
            module_id: ModuleId(0),
            def_id: defs.module_scope.values.get(&sym("main")).unwrap(),
        };
        let calls = GlobalDefId {
            module_id: ModuleId(0),
            def_id: defs.module_scope.values.get(&sym("calls")).unwrap(),
        };

        let edges = db.query(ExecutableValueRefEdgesQuery(main));

        assert!(edges.globals.contains(&calls), "{:?}", edges.globals);
    }

    #[test]
    fn module_defs_query_uses_active_item_tree_query() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "pub struct S { value: i32 }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(ModuleDefsQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "module_defs" && dependency.to.name == "active_module_item_tree"
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
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "struct S { value: i32 } extend S { pub fn make(value: i32) S { { value: value } } }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(ExtensionProviderValidationFactsQuery(ModuleId(0)));
        let _ = db.query(ExtensionProviderModuleFactsQuery(ModuleId(0)));
        let _ = db.query(ExtensionMethodIndexQuery);
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
            "extension_provider_discovery_index"
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
        assert!(!trace_has_dependency(
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
    fn extension_provider_module_facts_are_cached_across_body_updates() {
        let database = CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(
            vec![loaded_module(
                ModuleId(0),
                "main.nia",
                "struct S { value: i32 } extend S { pub fn make(value: i32) S { { value: value } } }",
            )],
        )));

        let _ = database.db.query(ExtensionMethodIndexQuery);
        let before_update = database.query_trace();
        assert!(
            query_executions(&before_update, "extension_provider_module_facts") > 0,
            "{before_update:?}"
        );

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module_with_revision(
                ModuleId(0),
                "main.nia",
                "struct S { value: i32 } extend S { pub fn make(value: i32) S { let next = value; { value: next } } }",
                SourceRevision(1),
            ),
        ])));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(
            !invalidated.contains(&"extension_provider_module_facts"),
            "{invalidated:?}"
        );
        assert!(
            !invalidated.contains(&"extension_provider_summary"),
            "{invalidated:?}"
        );
        let before_second_query = database.query_trace();

        let _ = database.db.query(ExtensionMethodIndexQuery);
        let after_second_query = database.query_trace();

        assert_query_executions_unchanged(
            &before_second_query,
            &after_second_query,
            "extension_provider_summary",
        );
        assert_query_executions_unchanged(
            &before_second_query,
            &after_second_query,
            "extension_provider_module_facts",
        );
    }

    #[test]
    fn body_sensitive_resolution_uses_full_active_item_tree_query() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "fn main() i32 { let value = 1; value }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(ValueResolutionQuery(ModuleId(0)));
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
        let loaded = loaded_program_with_modules(vec![
            loaded_module(
                ModuleId(0),
                "main.nia",
                r#"
module helper;

fn main() i32 {
    helper::value()
}
"#,
            ),
            loaded_module(ModuleId(1), "helper.nia", "pub fn value() i32 { 1 }"),
        ]);
        let db = query_db(loaded);

        let values = db.query(ValueResolutionQuery(ModuleId(0)));
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
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
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
        )]);
        let db = query_db(loaded);

        let values = db.query(ValueResolutionQuery(ModuleId(0)));
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
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "fn main() i32 { return 1; }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(FlowCheckQuery(ModuleId(0)));
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
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "static mut global: i32 = 1; fn main() i32 { global }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(StaticCheckQuery(ModuleId(0)));
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
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "struct S { value: i32 } static mut global: i32 = 1; fn main() i32 { global }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(BodyCheckQuery(ModuleId(0)));
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
    fn body_check_imports_full_lowering_types_before_working_interner_lookup() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
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
        )]);
        let db = query_db(loaded);

        let checked = db.query(BodyCheckQuery(ModuleId(0)));

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }

    #[test]
    fn visible_extensions_use_signature_type_normalization_and_nominal_provider_queries() {
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module facade;
using entry::facade;

fn main(value: facade::Used) i32 {
    value.len()
}
"#,
        );
        let facade = loaded_module(
            ModuleId(1),
            "facade.nia",
            r#"
module impls;
module types;

pub using self::types::Used;
"#,
        );
        let impls = loaded_module(
            ModuleId(2),
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
        let types = loaded_module(ModuleId(3), "facade/types.nia", "pub struct Used {}");
        let mut graph = ModuleGraph::with_symbol_text(main.path.clone(), Arc::new(test_symbols()));
        intern_child(&mut graph, main.id, "facade", nia_ids::Visibility::Public);
        intern_child(&mut graph, facade.id, "impls", nia_ids::Visibility::Private);
        intern_child(&mut graph, facade.id, "types", nia_ids::Visibility::Public);
        let loaded = LoadedProgram {
            graph,
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, facade, impls, types],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let _ = db.query(VisibleExtensionsQuery(ModuleId(0)));
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
                && dependency.to.description.contains("ModuleId(2)")
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
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "const VALUE = 1; fn main() i32 { VALUE }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(ConstQuery(ModuleId(0)));
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
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "pub fn main() i32 { 1 }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(MonomorphizationQuery);
        let trace = db.query_trace();

        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "monomorphization"
                && dependency.to.name == "program_trait_solving_signatures"
        }));
        assert!(!depends_on_body_signature_query(&trace, "monomorphization"));
    }

    #[test]
    fn executable_reachability_uses_lazy_signature_resolvers() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "pub fn main() i32 { 1 }",
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let _ = db.query(ExecutableCheckedModulesQuery);
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
        let loaded = loaded_program_with_modules(vec![
            loaded_module(
                ModuleId(0),
                "main.nia",
                "module providers; fn main() i32 { 1 }",
            ),
            loaded_module(
                ModuleId(1),
                "providers.nia",
                "struct S {} extend S { pub fn make() S { {} } }",
            ),
        ]);
        let db = query_db(loaded);

        let checked = db.query(BodyCheckQuery(ModuleId(0)));
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
        let loaded = loaded_program_with_modules(vec![
            loaded_module(
                ModuleId(0),
                "main.nia",
                "module module1; using self::module1::S; fn main() i32 { let s = S::make(); 1 }",
            ),
            loaded_module(
                ModuleId(1),
                "module1.nia",
                "pub struct S {} extend S { pub fn make() S { {} } }",
            ),
        ]);
        let db = query_db(loaded);

        let checked = db.query(BodyCheckQuery(ModuleId(0)));
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
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "trait Show { fn show(self) i32; } extend i32 : Show { fn show(self) i32 { self } } pub fn main() i32 { 1.show() }",
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let checked = db.query(CodegenProgramQuery);
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
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "extend ! { fn nope(self) void {} } pub fn main() i32 { 1 }",
        )]);
        let db = query_db(loaded);

        let checked = db.query(EntryCheckedProgramQuery);
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
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "extend ! { fn nope(self) void {} } pub fn main() i32 { 1 }",
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let checked = db.query(EntryCheckedProgramQuery);
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
        let main = loaded_module(
            ModuleId(0),
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
        let facade = loaded_module(
            ModuleId(1),
            "facade.nia",
            r#"
module args_impl;
module init_impl;
module types;

pub using self::types::{Args, ArgsIter, Init};
"#,
        );
        let init_impl = loaded_module(
            ModuleId(2),
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
        let args_impl = loaded_module(
            ModuleId(3),
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
        let types = loaded_module(
            ModuleId(4),
            "facade/types.nia",
            r#"
pub struct Init {}
pub struct Args {}
pub struct ArgsIter {}
"#,
        );
        let mut graph = ModuleGraph::with_symbol_text(main.path.clone(), Arc::new(test_symbols()));
        intern_child(&mut graph, main.id, "facade", nia_ids::Visibility::Public);
        intern_child(
            &mut graph,
            facade.id,
            "args_impl",
            nia_ids::Visibility::Private,
        );
        intern_child(
            &mut graph,
            facade.id,
            "init_impl",
            nia_ids::Visibility::Private,
        );
        intern_child(&mut graph, facade.id, "types", nia_ids::Visibility::Public);
        let loaded = LoadedProgram {
            graph,
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, facade, init_impl, args_impl, types],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let checked = db.query(CodegenProgramQuery);

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }

    #[test]
    fn visible_extensions_do_not_expand_using_type_modules_as_provider_modules() {
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module facade;
using entry::facade;

fn main(value: facade::Used) i32 {
    value.len()
}
"#,
        );
        let facade = loaded_module(
            ModuleId(1),
            "facade.nia",
            r#"
module impls;
module types;

pub using self::types::{Unused, Used};
"#,
        );
        let impls = loaded_module(
            ModuleId(2),
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
        let types = loaded_module(
            ModuleId(3),
            "facade/types.nia",
            r#"
pub struct Unused {}
pub struct Used {}
"#,
        );
        let mut graph = ModuleGraph::with_symbol_text(main.path.clone(), Arc::new(test_symbols()));
        intern_child(&mut graph, main.id, "facade", nia_ids::Visibility::Public);
        intern_child(&mut graph, facade.id, "impls", nia_ids::Visibility::Private);
        intern_child(&mut graph, facade.id, "types", nia_ids::Visibility::Public);
        let loaded = LoadedProgram {
            graph,
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, facade, impls, types],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let checked = db.query(CodegenProgramQuery);

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let trace = db.query_trace();
        assert!(
            !trace.dependencies.iter().any(|dependency| {
                dependency.from.name == "visible_extensions"
                    && dependency.from.description.contains("ModuleId(0)")
                    && dependency.to.description.contains("ModuleId(3)")
                    && dependency.to.name == "signature_type_normalization"
            }),
            "visible extensions should not normalize every module that merely defines a using-imported type"
        );
    }

    #[test]
    fn visible_trait_impls_follow_facade_reexport_item_modules() {
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module fmt;
using entry::fmt;

fn main() i32 {
    fmt::parse[i32](&"abc")
}
"#,
        );
        let fmt = loaded_module(
            ModuleId(1),
            "fmt.nia",
            r#"
pub module parse_impl;
pub using parse_impl::{ParseFrom, parse};
"#,
        );
        let parse_impl = loaded_module(
            ModuleId(2),
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
        let mut graph = ModuleGraph::with_symbol_text(main.path.clone(), Arc::new(test_symbols()));
        intern_child(&mut graph, main.id, "fmt", nia_ids::Visibility::Public);
        intern_child(
            &mut graph,
            fmt.id,
            "parse_impl",
            nia_ids::Visibility::Public,
        );
        let loaded = LoadedProgram {
            graph,
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::Bare,
            modules: vec![main, fmt, parse_impl],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let trait_impls = db.query(VisibleTraitImplsQuery(ModuleId(0)));

        assert_eq!(trait_impls.trait_impls.len(), 2);
        assert!(
            trait_impls
                .trait_impls
                .iter()
                .all(|impl_signature| impl_signature.module_id == ModuleId(2)),
            "{:?}",
            trait_impls.trait_impls
        );
    }

    #[test]
    fn executable_reachability_keeps_matched_trait_impl_method_bodies() {
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module parse;
using entry::parse;

pub fn main() i32 {
    parse::parse[i32, parse::Input](parse::Input {})
}
"#,
        );
        let parse = loaded_module(
            ModuleId(1),
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
        let mut graph = ModuleGraph::with_symbol_text(main.path.clone(), Arc::new(test_symbols()));
        intern_child(&mut graph, main.id, "parse", nia_ids::Visibility::Public);
        let loaded = LoadedProgram {
            graph,
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, parse],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let checked = db.query(ExecutableCheckedModulesQuery);
        let parse_module = checked
            .iter()
            .find(|module| module.id == ModuleId(1))
            .expect("parse module should be executable-reachable");
        let parse_from = parse_module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == sym("parse_from") && def.kind == nia_defs::DefKind::Method).then_some(
                    GlobalDefId {
                        module_id: ModuleId(1),
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
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "const fn value() usize { 1 } const VALUE = value();",
        )]);
        let db = query_db(loaded);

        let _ = db.query(ConstModuleQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "const_module"
                && dependency.to.name == "full_active_module_item_tree"
        }));
    }

    #[test]
    fn semantic_use_table_query_combines_value_local_and_type_resolution() {
        let source = "static VALUE: i32 = 1; fn main() i32 { let mut local: i32 = VALUE; local }";
        let loaded =
            loaded_program_with_modules(vec![loaded_module(ModuleId(0), "main.nia", source)]);
        let db = query_db(loaded);

        let table = db.query(SemanticUseTableQuery(ModuleId(0)));
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
    fn checked_module_exposes_semantic_use_table_product() {
        let source = "fn main() i32 { let mut local: i32 = 1; local }";
        let checked =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(ModuleId(0), "main.nia", source),
            ])))
            .check_program();

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let module = checked.modules.first().expect("checked module");
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
    fn backend_lowering_uses_executable_checked_module_body_ir() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "fn main() i32 { static value: i32 = 1; value }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(BackendLoweringQuery);
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
    fn executable_checked_modules_reuse_filtered_const_inputs() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
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
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
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
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
const explicit: usize = 19usize;
const inferred = 4usize;

fn main() usize {
    explicit + inferred
}
"#,
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
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
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
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
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
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
        let main = loaded_module(
            ModuleId(0),
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
        let facade = loaded_module(
            ModuleId(1),
            "facade.nia",
            r#"
module raw;
using self::raw;

pub const LEN: usize = raw::LEN;
"#,
        );
        let raw = loaded_module(
            ModuleId(2),
            "facade/raw.nia",
            r#"
pub const LEN: usize = 4usize;
"#,
        );
        let mut graph = ModuleGraph::with_symbol_text(main.path.clone(), Arc::new(test_symbols()));
        intern_child(&mut graph, main.id, "facade", nia_ids::Visibility::Public);
        intern_child(&mut graph, facade.id, "raw", nia_ids::Visibility::Public);
        let loaded = LoadedProgram {
            graph,
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, facade, raw],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let entry = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
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
        let main = loaded_module(
            ModuleId(0),
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
        let raw = loaded_module(
            ModuleId(1),
            "raw.nia",
            r#"
pub const LEN: usize = 4usize;
"#,
        );
        let mut graph = ModuleGraph::with_symbol_text(main.path.clone(), Arc::new(test_symbols()));
        intern_child(&mut graph, main.id, "raw", nia_ids::Visibility::Public);
        let loaded = LoadedProgram {
            graph,
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, raw],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let entry = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
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
        let main = loaded_module(
            ModuleId(0),
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
        let writer = loaded_module(
            ModuleId(1),
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
        let mut graph = ModuleGraph::with_symbol_text(main.path.clone(), Arc::new(test_symbols()));
        intern_child(&mut graph, main.id, "writer", nia_ids::Visibility::Public);
        let loaded = LoadedProgram {
            graph,
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, writer],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let writer = modules
            .iter()
            .find(|module| module.id == ModuleId(1))
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
            module_id: ModuleId(1),
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
            !matches!(writer.body_ir.interner.get(self_ty), Some(TyKind::Error)),
            "reachable extension method receiver/params should not collapse to error types"
        );
    }

    #[test]
    fn trait_signature_subset_resolves_local_extend_target_types() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
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
        )]);
        let db = query_db(loaded);

        let signatures = db.query_shared(SignatureItemSignaturesQuery(
            ModuleId(0),
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let lowering = db.query(SignatureTypeLoweringQuery(
            ModuleId(0),
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let impl_signature = signatures
            .trait_impls
            .iter()
            .find(|impl_signature| !impl_signature.methods.is_empty())
            .expect("trait impl should be collected");

        assert!(
            !matches!(
                lowering.interner.get(impl_signature.target_ty),
                Some(TyKind::Error)
            ),
            "trait signature subset should resolve local extend target types"
        );
    }

    #[test]
    fn trait_signature_subset_resolves_imported_extend_target_types() {
        let main = loaded_module(
            ModuleId(0),
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
        let platform = loaded_module(
            ModuleId(1),
            "platform.nia",
            r#"
pub enum Errno: i32 {
    Bad = 1,
    _,
}
"#,
        );
        let mut graph = ModuleGraph::with_symbol_text(main.path.clone(), Arc::new(test_symbols()));
        intern_child(&mut graph, main.id, "platform", nia_ids::Visibility::Public);
        let loaded = LoadedProgram {
            graph,
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, platform],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let signatures = db.query_shared(SignatureItemSignaturesQuery(
            ModuleId(0),
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let lowering = db.query(SignatureTypeLoweringQuery(
            ModuleId(0),
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let impl_signature = signatures
            .trait_impls
            .iter()
            .find(|impl_signature| !impl_signature.methods.is_empty())
            .expect("trait impl should be collected");

        assert!(
            !matches!(
                lowering.interner.get(impl_signature.target_ty),
                Some(TyKind::Error)
            ),
            "trait signature subset should resolve imported extend target types"
        );
    }

    #[test]
    fn trait_signature_subset_resolves_reexported_extend_target_types() {
        let main = loaded_module(
            ModuleId(0),
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
        let platform = loaded_module(
            ModuleId(1),
            "platform.nia",
            r#"
module types;
using entry::platform::types;

pub using types::{Errno};
"#,
        );
        let types = loaded_module(
            ModuleId(2),
            "types.nia",
            r#"
pub enum Errno: i32 {
    Bad = 1,
    _,
}
"#,
        );
        let mut graph = ModuleGraph::with_symbol_text(main.path.clone(), Arc::new(test_symbols()));
        intern_child(&mut graph, main.id, "platform", nia_ids::Visibility::Public);
        intern_child(
            &mut graph,
            platform.id,
            "types",
            nia_ids::Visibility::Public,
        );
        let loaded = LoadedProgram {
            graph,
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, platform, types],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let signatures = db.query_shared(SignatureItemSignaturesQuery(
            ModuleId(0),
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let lowering = db.query(SignatureTypeLoweringQuery(
            ModuleId(0),
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let impl_signature = signatures
            .trait_impls
            .iter()
            .find(|impl_signature| !impl_signature.methods.is_empty())
            .expect("trait impl should be collected");

        assert!(
            !matches!(
                lowering.interner.get(impl_signature.target_ty),
                Some(TyKind::Error)
            ),
            "trait signature subset should resolve re-exported extend target types"
        );
    }

    #[test]
    fn executable_incremental_body_check_preserves_reexported_trait_witness_receiver_types() {
        let main = loaded_module(
            ModuleId(0),
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
        let platform = loaded_module(
            ModuleId(1),
            "platform.nia",
            r#"
module types;
using entry::platform::types;

pub using types::{Errno};
"#,
        );
        let types = loaded_module(
            ModuleId(2),
            "types.nia",
            r#"
pub enum Errno: i32 {
    Bad = 1,
    _,
}
"#,
        );
        let mut graph = ModuleGraph::with_symbol_text(main.path.clone(), Arc::new(test_symbols()));
        intern_child(&mut graph, main.id, "platform", nia_ids::Visibility::Public);
        intern_child(
            &mut graph,
            platform.id,
            "types",
            nia_ids::Visibility::Public,
        );
        let loaded = LoadedProgram {
            graph,
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, platform, types],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
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
                        module_id: ModuleId(0),
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
            !matches!(module.body_ir.interner.get(self_ty), Some(TyKind::Error)),
            "re-exported trait witness receiver should not collapse to error"
        );
    }

    #[test]
    fn executable_reachability_expands_where_predicates_through_generic_extension_wrappers() {
        let main = loaded_module(
            ModuleId(0),
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
        let error = loaded_module(
            ModuleId(1),
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
        let facade = loaded_module(
            ModuleId(2),
            "facade.nia",
            r#"
using entry::error;
"#,
        );
        let mut graph = ModuleGraph::with_symbol_text(main.path.clone(), Arc::new(test_symbols()));
        intern_child(&mut graph, main.id, "error", nia_ids::Visibility::Public);
        intern_child(&mut graph, main.id, "facade", nia_ids::Visibility::Public);
        let loaded = LoadedProgram {
            graph,
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, error, facade],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let into_error = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == sym("into_error") && def.kind == nia_defs::DefKind::Method).then_some(
                    GlobalDefId {
                        module_id: ModuleId(0),
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
        let main = loaded_module(
            ModuleId(0),
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
        let error = loaded_module(
            ModuleId(1),
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
        let impls = loaded_module(
            ModuleId(2),
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
        let mut graph = ModuleGraph::with_symbol_text(main.path.clone(), Arc::new(test_symbols()));
        intern_child(&mut graph, main.id, "error", nia_ids::Visibility::Public);
        intern_child(&mut graph, main.id, "impls", nia_ids::Visibility::Public);
        let loaded = LoadedProgram {
            graph,
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, error, impls],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(2))
            .expect("impl module should be executable-reachable");
        let into_error = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == sym("into_error") && def.kind == nia_defs::DefKind::Method).then_some(
                    GlobalDefId {
                        module_id: ModuleId(2),
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
        let main = loaded_module(
            ModuleId(0),
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
        let error = loaded_module(
            ModuleId(1),
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
        let impls = loaded_module(
            ModuleId(2),
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
        let mut graph = ModuleGraph::with_symbol_text(main.path.clone(), Arc::new(test_symbols()));
        intern_child(&mut graph, main.id, "error", nia_ids::Visibility::Public);
        intern_child(&mut graph, main.id, "impls", nia_ids::Visibility::Public);
        let loaded = LoadedProgram {
            graph,
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, error, impls],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(2))
            .expect("impl module should be executable-reachable");
        let into_error = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == sym("into_error") && def.kind == nia_defs::DefKind::Method).then_some(
                    GlobalDefId {
                        module_id: ModuleId(2),
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
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
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
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let next = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Method && def.name == sym("next")).then_some(
                    GlobalDefId {
                        module_id: ModuleId(0),
                        def_id,
                    },
                )
            })
            .expect("Iterator witness method");

        assert!(
            module.body_ir.function_bodies.contains_key(&next),
            "executable body checking must include builtin trait witness bodies"
        );
    }

    #[test]
    fn executable_checked_modules_do_not_body_check_unmatched_builtin_trait_witnesses() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
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
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let unused_next = module
            .defs
            .defs
            .iter()
            .filter_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Method && def.name == sym("next")).then_some(
                    GlobalDefId {
                        module_id: ModuleId(0),
                        def_id,
                    },
                )
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
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
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
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let unused = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Method && def.name == sym("unused")).then_some(
                    GlobalDefId {
                        module_id: ModuleId(0),
                        def_id,
                    },
                )
            })
            .expect("unused witness method");

        assert!(
            !module.body_ir.function_bodies.contains_key(&unused),
            "executable body checking should not include unused trait witness bodies"
        );
    }

    #[test]
    fn executable_checked_modules_include_trait_witnesses_required_by_generic_where_predicates() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
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
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let into_error_methods = module
            .defs
            .defs
            .iter()
            .filter_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Method && def.name == sym("into_error")).then_some(
                    GlobalDefId {
                        module_id: ModuleId(0),
                        def_id,
                    },
                )
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
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
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
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let checked_witness_names = module
            .defs
            .defs
            .iter()
            .filter_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Method
                    && module.body_ir.function_bodies.contains_key(&GlobalDefId {
                        module_id: ModuleId(0),
                        def_id,
                    }))
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
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
static unused = missing_symbol;

fn main() i32 {
    0
}
"#,
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let unused = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Global && def.name == sym("unused")).then_some(
                    GlobalDefId {
                        module_id: ModuleId(0),
                        def_id,
                    },
                )
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
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
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
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        assert!(
            module.layouts.diagnostics.is_empty(),
            "unreachable recursive aggregate should not force layout diagnostics: {:?}",
            module.layouts.diagnostics
        );

        let backend_lowering = db.query(BackendLoweringQuery);
        let backend_module = backend_lowering
            .program
            .modules
            .iter()
            .find(|module| module.id == ModuleId(0))
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
    fn executable_backend_lowering_imports_external_extension_owner_where_predicates() {
        let main = loaded_module(
            ModuleId(0),
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
        let ext = loaded_module(
            ModuleId(1),
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
        let bounds = loaded_module(
            ModuleId(2),
            "bounds.nia",
            r#"
pub trait Marker {}

pub struct Token {}

extend Token : Marker {}
"#,
        );
        let mut graph = ModuleGraph::with_symbol_text(main.path.clone(), Arc::new(test_symbols()));
        intern_child(&mut graph, main.id, "ext", nia_ids::Visibility::Public);
        intern_child(&mut graph, main.id, "bounds", nia_ids::Visibility::Public);
        let loaded = LoadedProgram {
            graph,
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, ext, bounds],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let backend_lowering = db.query(BackendLoweringQuery);

        assert!(
            backend_lowering.diagnostics.is_empty(),
            "backend lowering should import external extension owner predicates without diagnostics: {:?}",
            backend_lowering.diagnostics
        );
    }

    #[test]
    fn executable_backend_lowering_includes_cross_module_trait_default_vtable_instances() {
        let main = loaded_module(
            ModuleId(0),
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
        let traits = loaded_module(
            ModuleId(1),
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
        let impls = loaded_module(
            ModuleId(2),
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
        let mut graph = ModuleGraph::with_symbol_text(main.path.clone(), Arc::new(test_symbols()));
        intern_child(&mut graph, main.id, "module1", nia_ids::Visibility::Public);
        intern_child(&mut graph, main.id, "module2", nia_ids::Visibility::Public);
        let loaded = LoadedProgram {
            graph,
            symbols: test_symbols(),
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, traits, impls],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let backend_lowering = db.query(BackendLoweringQuery);

        assert!(
            backend_lowering.diagnostics.is_empty(),
            "backend lowering should not report diagnostics: {:?}",
            backend_lowering.diagnostics
        );
        let vtable_instance_refs = backend_lowering
            .program
            .modules
            .iter()
            .flat_map(|module| {
                module
                    .trait_object_vtables
                    .iter()
                    .flat_map(move |vtable| vtable.entries.iter().map(move |entry| (module, entry)))
            })
            .filter_map(|(module, entry)| match &entry.function {
                nia_backend_ir::BackendTraitObjectVtableFunction::FunctionInstance {
                    def_id,
                    arg_module_id,
                    self_arg,
                    args,
                    const_args,
                } => Some((
                    module,
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
        for (vtable_module, def_id, arg_module_id, self_arg, args, const_args) in
            vtable_instance_refs
        {
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
                .filter(|(instance_module, instance)| {
                    backend_function_instance_matches_vtable_ref(
                        VtableFunctionInstanceRef {
                            module: vtable_module,
                            def_id,
                            arg_module_id,
                            self_arg,
                            args: &args,
                            const_args: &const_args,
                        },
                        instance_module,
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
        let entry = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module types;
using entry::types;

fn main(value: types::Used) i32 {
    value.value
}
"#,
        );
        let types = loaded_module(
            ModuleId(1),
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
        let mut loaded = loaded_program_with_entry_child(entry, "types", types);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let type_module = modules
            .iter()
            .find(|module| module.id == ModuleId(1))
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
                    && query.frame.description.contains("ModuleId(1)")
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
                    && query.frame.description.contains("ModuleId(1)")
                    && query.frame.description.contains("Types")
                    && query.stats.executions > 0
            }),
            "type-only module should use signature type lowering: {:?}",
            trace
                .queries
                .iter()
                .filter(|query| query.frame.description.contains("ModuleId(1)"))
                .collect::<Vec<_>>()
        );
        for full_query in ["type_resolution", "type_lowering", "value_resolution"] {
            assert!(
                !trace.queries.iter().any(|query| {
                    query.frame.name == full_query
                        && query.frame.description.contains("ModuleId(1)")
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
        let entry = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module types;
using entry::types;

fn main(value: types::Mode) i32 {
    0
}
"#,
        );
        let types = loaded_module(
            ModuleId(1),
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
        let mut loaded = loaded_program_with_entry_child(entry, "types", types);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let type_module = modules
            .iter()
            .find(|module| module.id == ModuleId(1))
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
                        && query.frame.description.contains("ModuleId(1)")
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
        let entry = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module types;
using entry::types;

fn main(value: types::Packet) i32 {
    0
}
"#,
        );
        let types = loaded_module(
            ModuleId(1),
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
        let mut loaded = loaded_program_with_entry_child(entry, "types", types);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let type_module = modules
            .iter()
            .find(|module| module.id == ModuleId(1))
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
                        && query.frame.description.contains("ModuleId(1)")
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
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module helper;
using entry::helper;

fn main() i32 {
    helper::id[i32](1)
}
"#,
        );
        let helper = loaded_module(
            ModuleId(1),
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
        let mut loaded = loaded_program_with_entry_child(main, "helper", helper);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let helper_module = modules
            .iter()
            .find(|module| module.id == ModuleId(1))
            .expect("called generic function owner should be executable-reachable");
        let unused_bad = helper_module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Function && def.name == sym("unused_bad"))
                    .then_some(GlobalDefId {
                        module_id: ModuleId(1),
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
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
static used: i32 = 1;

fn main() i32 {
    used
}
"#,
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let used = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Global && def.name == sym("used")).then_some(
                    GlobalDefId {
                        module_id: ModuleId(0),
                        def_id,
                    },
                )
            })
            .expect("used global");

        assert!(
            module.body_ir.global_inits.contains_key(&used),
            "reachable global initializers must be retained for executable codegen"
        );
    }

    #[test]
    fn executable_checked_modules_include_reachable_local_static_initializers() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
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
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let text = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Global && def.name == sym("text")).then_some(
                    GlobalDefId {
                        module_id: ModuleId(0),
                        def_id,
                    },
                )
            })
            .expect("local static global");

        assert!(
            module.body_ir.global_inits.contains_key(&text),
            "reachable local static initializers must be retained for executable codegen"
        );
    }

    #[test]
    fn executable_checked_modules_include_reachable_extension_method_local_static_initializers() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
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
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let o2 = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Global && def.name == sym("o2")).then_some(
                    GlobalDefId {
                        module_id: ModuleId(0),
                        def_id,
                    },
                )
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
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
using helper::Mode;

fn main() i32 {
    _ = Mode::O2.argv();
    0
}
"#,
        );
        let helper = loaded_module(
            ModuleId(1),
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
        let mut loaded = loaded_program_with_entry_child(main, "helper", helper);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(1))
            .expect("helper module should be executable-reachable");
        let o2 = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Global && def.name == sym("o2")).then_some(
                    GlobalDefId {
                        module_id: ModuleId(1),
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

        let backend = db.query(BackendLoweringQuery);
        let backend_module = backend
            .program
            .modules
            .iter()
            .find(|module| module.id == ModuleId(1))
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
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
fn unused() i32 {
}

fn main() i32 {
    0
}
"#,
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");

        assert!(
            module.flow_check.diagnostics.is_empty(),
            "unreachable function flow diagnostics should not block executable checking: {:?}",
            module.flow_check.diagnostics
        );
    }

    #[test]
    fn executable_checked_modules_do_not_body_check_unreachable_loaded_modules() {
        let entry = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
pub module unused;

fn main() i32 {
    0
}
"#,
        );
        let unused = loaded_module(
            ModuleId(1),
            "unused.nia",
            r#"
pub fn expensive_or_invalid() i32 {
    missing_symbol
}
"#,
        );
        let mut loaded = loaded_program_with_entry_child(entry, "unused", unused);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let trace = db.query_trace();

        assert!(
            modules.iter().all(|module| module.id != ModuleId(1)),
            "unreachable module should not be kept for executable codegen"
        );
        assert!(
            !trace.queries.iter().any(|query| {
                query.frame.name == "body_check"
                    && query.frame.description.contains("ModuleId(1)")
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
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "const N: usize = 4; fn main() i32 { let mut values: [N]i32 = [0; N]; values.len() as i32 }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(BodyCheckQuery(ModuleId(0)));
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
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "pub struct S { value: i32 } fn main() i32 { 0 }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(TypeResolutionQuery(ModuleId(0)));
        let invalidation = db.invalidate(ModuleDefsQuery(ModuleId(0)));
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

        let _ = db.query(TypeResolutionQuery(ModuleId(0)));
    }

    #[test]
    fn invalidates_module_defs_after_item_tree_changes() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "pub struct S { value: i32 }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(ModuleDefsQuery(ModuleId(0)));
        let invalidation = db.invalidate(ModuleItemTreeInputQuery(ModuleId(0)));
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
