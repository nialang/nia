// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    CheckedModule, CheckedProgram, LoadedModule, LoadedProgram, ProgramDiagnostic, RuntimeModel,
    TimingMode, module_diagnostics,
    program_signatures::{
        ExtensionModuleInput, ExtensionModuleItemInput, ModuleSignatureInput,
        VisibleExtensionsForModule, VisibleExtensionsInput, collect_extension_associated_values,
        collect_extension_methods, collect_program_comptimes, collect_program_enums,
        collect_program_functions, collect_program_globals, collect_program_structs,
        collect_program_traits, collect_program_unions, visible_extensions_for_module,
    },
    public_surface::compute_public_surfaces,
};
use nia_backend_lower::BackendLowerModuleInput;
use nia_comptime_check::{ComptimeCheck, ComptimeModuleLowering};
use nia_comptime_ir::ResolvedComptimeModule;
use nia_defs::{DefCollection, ModuleUsingScope, PublicSurfaces};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, ModuleId};
use nia_imports::ModuleGraph;
use nia_item_signatures::{
    ItemSignatures, ProgramComptimeSignature, ProgramEnumSignature, ProgramFunctionSignature,
    ProgramGlobalSignature, ProgramSignatureMaps, ProgramStructSignature, ProgramTraitSignature,
    ProgramUnionSignature,
};
use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
use nia_local_resolve::LocalResolution;
use nia_monomorphize::MonomorphizeModuleInput;
use nia_node_id::NodeOriginTable;
use nia_opt::{NiaOptimizationLevel, OptimizationPolicy};
use nia_parser::ParseError;
use nia_query::{QueryDb, QueryError, QueryFrame, QueryKey, QueryTrace};
use nia_source::{SourcePath, SourceVersion};
use nia_span::Span;
use nia_target_config::TargetConfig;
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
mod program;
mod providers;
mod resolve;
mod types;

use backend_lowering::*;
use base::*;
use checked::*;
use checks::*;
use diagnostics::*;
use program::*;
use providers::*;
use resolve::*;
use types::*;

type ProgramDefsById = Arc<HashMap<ModuleId, DefCollection>>;
type ProgramTypeLowerings = Arc<HashMap<ModuleId, TypeLowering>>;
type ProgramItemSignaturesById = Arc<HashMap<ModuleId, ItemSignatures>>;
type ProgramTypeNormalizations = Arc<HashMap<ModuleId, TypeNormalization>>;
type ProgramComptimeModules = Arc<HashMap<ModuleId, ResolvedComptimeModule>>;
type ProgramComptimeById = Arc<HashMap<ModuleId, ComptimeCheck>>;
type ProgramSignaturesValue = Arc<ProgramSignatures>;
type ExtensionMethodsValue = Arc<ExtensionMethodsQueryValue>;
type VisibleExtensionsValue = Arc<VisibleExtensionsForModule>;

#[derive(Debug, Clone)]
pub struct CompileRequest {
    pub loaded: LoadedProgram,
    pub optimization: NiaOptimizationLevel,
    pub timings: TimingMode,
}

impl CompileRequest {
    pub fn new(loaded: LoadedProgram) -> Self {
        Self {
            loaded,
            optimization: NiaOptimizationLevel::default(),
            timings: TimingMode::Off,
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
            .loaded
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
        if diff.graph_changed {
            invalidation.extend(self.db.invalidate(ModuleGraphQuery));
        }
        if diff.loaded_modules_changed {
            invalidation.extend(self.db.invalidate(LoadedModulesQuery));
        }
        if diff.program_diagnostics_changed {
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
        if diff.timings_changed {
            invalidation.extend(self.db.invalidate(CompilerTimingsQuery));
        }
        for module in diff.changed_modules {
            if module.path {
                invalidation.extend(self.db.invalidate(ModulePathQuery(module.id)));
            }
            if module.source_version {
                invalidation.extend(self.db.invalidate(ModuleSourceVersionQuery(module.id)));
            }
            if module.module_ast {
                invalidation.extend(self.db.invalidate(ModuleAstQuery(module.id)));
            }
            if module.full_item_tree {
                invalidation.extend(self.db.invalidate(FullModuleItemTreeInputQuery(module.id)));
            }
            if module.origins {
                invalidation.extend(self.db.invalidate(ModuleOriginsQuery(module.id)));
            }
            if module.parse_errors {
                invalidation.extend(self.db.invalidate(ModuleParseErrorsQuery(module.id)));
            }
            if module.item_tree {
                invalidation.extend(self.db.invalidate(ModuleItemTreeInputQuery(module.id)));
            }
            if module.active_item_tree {
                invalidation.extend(
                    self.db
                        .invalidate(ActiveModuleItemTreeInputQuery(module.id)),
                );
            }
            if module.full_active_item_tree {
                invalidation.extend(
                    self.db
                        .invalidate(FullActiveModuleItemTreeInputQuery(module.id)),
                );
            }
        }
        invalidation
    }
}

impl std::fmt::Debug for CompilerDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        f.debug_struct("CompilerDatabase")
            .field("graph", &inputs.loaded.graph)
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
    let inputs = Arc::new(RwLock::new(CompilerInputs::new(request)));
    let db = QueryDb::new(CompilerContext {
        inputs: inputs.clone(),
        providers,
    });
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
        monomorphization: nia_monomorphize::Monomorphization {
            instances: Vec::new(),
            type_interners: std::collections::HashMap::new(),
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
            Diagnostic::user_error_at("E0201", Span::default(), message)
        }
        QueryError::InvalidInput { query, message } => Diagnostic::user_error_at(
            "E0201",
            Span::default(),
            format!("invalid query input for {}: {message}", query.description),
        ),
    }
}

struct CompilerContext {
    inputs: Arc<RwLock<CompilerInputs>>,
    providers: CompilerQueryProviders,
}

#[derive(Debug, Clone)]
struct CompilerInputs {
    loaded: LoadedProgram,
    modules_by_id: HashMap<ModuleId, usize>,
    target: TargetConfig,
    runtime: crate::RuntimeModel,
    optimization: OptimizationPolicy,
    timings: TimingMode,
}

impl CompilerInputs {
    fn new(request: CompileRequest) -> Self {
        let loaded = request.loaded;
        let modules_by_id = index_loaded_modules(&loaded);
        Self {
            target: loaded.target.clone(),
            runtime: loaded.runtime,
            loaded,
            modules_by_id,
            optimization: request.optimization.policy(),
            timings: request.timings,
        }
    }
}

impl CompilerContext {
    fn module_field<T, K>(
        &self,
        db: &QueryDb<CompilerContext>,
        key: &K,
        module_id: ModuleId,
        field: impl FnOnce(&LoadedModule) -> T,
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
            .loaded
            .modules
            .iter()
            .map(|module| module.id)
            .collect()
    }

    fn loaded_module(&self, module_id: ModuleId) -> Option<LoadedModule> {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        inputs
            .modules_by_id
            .get(&module_id)
            .and_then(|index| inputs.loaded.modules.get(*index))
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

    fn module_ast(&self, db: &QueryDb<CompilerContext>, module_id: ModuleId) -> nia_ast::Module {
        self.module_field(db, &ModuleAstQuery(module_id), module_id, |module| {
            module.module.clone()
        })
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
    ) -> ModuleItemTree {
        self.module_field(
            db,
            &ModuleItemTreeInputQuery(module_id),
            module_id,
            |module| module.item_tree.clone(),
        )
    }

    fn active_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> ActiveModuleItemTree {
        self.module_field(
            db,
            &ActiveModuleItemTreeInputQuery(module_id),
            module_id,
            |module| module.active_item_tree.clone(),
        )
    }

    fn path_for_module(&self, module_id: ModuleId) -> SourcePath {
        self.loaded_module(module_id)
            .map(|module| module.path.clone())
            .unwrap_or_else(|| SourcePath::new("<unknown>"))
    }

    fn module_graph(&self) -> ModuleGraph {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .loaded
            .graph
            .clone()
    }

    fn load_diagnostics(&self) -> Vec<ProgramDiagnostic> {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .loaded
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

fn index_loaded_modules(loaded: &LoadedProgram) -> HashMap<ModuleId, usize> {
    loaded
        .modules
        .iter()
        .enumerate()
        .map(|(index, module)| (module.id, index))
        .collect()
}

#[derive(Debug, Default)]
struct CompilerInputDiff {
    graph_changed: bool,
    loaded_modules_changed: bool,
    program_diagnostics_changed: bool,
    target_changed: bool,
    runtime_changed: bool,
    optimization_changed: bool,
    timings_changed: bool,
    changed_modules: Vec<ChangedModuleInput>,
}

impl CompilerInputDiff {
    fn between(old: &CompilerInputs, new: &CompilerInputs) -> Self {
        let changed_modules = changed_loaded_modules(old, new);
        Self {
            graph_changed: old.loaded.graph != new.loaded.graph,
            loaded_modules_changed: loaded_module_ids(old) != loaded_module_ids(new),
            program_diagnostics_changed: old.loaded.diagnostics != new.loaded.diagnostics,
            target_changed: old.target != new.target,
            runtime_changed: old.runtime != new.runtime,
            optimization_changed: old.optimization != new.optimization,
            timings_changed: old.timings != new.timings,
            changed_modules,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChangedModuleInput {
    id: ModuleId,
    path: bool,
    source_version: bool,
    module_ast: bool,
    origins: bool,
    parse_errors: bool,
    item_tree: bool,
    full_item_tree: bool,
    active_item_tree: bool,
    full_active_item_tree: bool,
}

impl ChangedModuleInput {
    fn between(
        module_id: ModuleId,
        old: Option<&LoadedModule>,
        new: Option<&LoadedModule>,
    ) -> Option<Self> {
        let changed = match (old, new) {
            (Some(old), Some(new)) => Self {
                id: module_id,
                path: old.path != new.path,
                source_version: old.source_version != new.source_version,
                module_ast: old.module != new.module,
                origins: old.origins != new.origins,
                parse_errors: old.parse_errors != new.parse_errors,
                item_tree: !old.item_tree.declaration_eq(&new.item_tree),
                full_item_tree: old.item_tree != new.item_tree,
                active_item_tree: !old.active_item_tree.declaration_eq(&new.active_item_tree),
                full_active_item_tree: old.active_item_tree != new.active_item_tree,
            },
            (Some(_), None) | (None, Some(_)) => Self {
                id: module_id,
                path: true,
                source_version: true,
                module_ast: true,
                origins: true,
                parse_errors: true,
                item_tree: true,
                full_item_tree: true,
                active_item_tree: true,
                full_active_item_tree: true,
            },
            (None, None) => return None,
        };
        if changed.path
            || changed.source_version
            || changed.module_ast
            || changed.origins
            || changed.parse_errors
            || changed.item_tree
            || changed.full_item_tree
            || changed.active_item_tree
            || changed.full_active_item_tree
        {
            Some(changed)
        } else {
            None
        }
    }
}

fn changed_loaded_modules(old: &CompilerInputs, new: &CompilerInputs) -> Vec<ChangedModuleInput> {
    let module_ids = old
        .loaded
        .modules
        .iter()
        .map(|module| module.id)
        .chain(new.loaded.modules.iter().map(|module| module.id))
        .collect::<HashSet<_>>();
    let mut changed = module_ids
        .into_iter()
        .filter_map(|module_id| {
            ChangedModuleInput::between(
                module_id,
                old.loaded_module(module_id),
                new.loaded_module(module_id),
            )
        })
        .collect::<Vec<_>>();
    changed.sort_by_key(|module| module.id.0);
    changed
}

fn loaded_module_ids(inputs: &CompilerInputs) -> Vec<ModuleId> {
    inputs
        .loaded
        .modules
        .iter()
        .map(|module| module.id)
        .collect()
}

impl CompilerInputs {
    fn loaded_module(&self, module_id: ModuleId) -> Option<&LoadedModule> {
        self.modules_by_id
            .get(&module_id)
            .and_then(|index| self.loaded.modules.get(*index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RuntimeModel;
    use nia_sema_ir::SemanticValueUse;
    use nia_source::{SourceId, SourceRevision};

    fn loaded_program_with_modules(modules: Vec<LoadedModule>) -> LoadedProgram {
        LoadedProgram {
            graph: ModuleGraph::new(SourcePath::new("main.nia")),
            target: TargetConfig::host(),
            runtime: RuntimeModel::Bare,
            modules,
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
        let (module, parse_errors) = nia_parser::parse_module(source);
        assert!(parse_errors.is_empty(), "{parse_errors:?}");
        let item_tree = ModuleItemTree::from_module(&module);
        LoadedModule {
            id,
            path: SourcePath::new(path),
            source_version: nia_source::SourceVersion {
                id: SourceId(id.0),
                revision,
            },
            source: source.to_string(),
            raw_module: module.clone(),
            module,
            item_tree: item_tree.clone(),
            active_item_tree: ActiveModuleItemTree::new(
                item_tree.items.clone(),
                Default::default(),
            ),
            parse_errors,
            origins: nia_node_id::NodeOriginTable::default(),
        }
    }

    fn query_db(loaded: LoadedProgram) -> QueryDb<CompilerContext> {
        let inputs = Arc::new(RwLock::new(CompilerInputs::new(CompileRequest::new(
            loaded,
        ))));
        QueryDb::new(CompilerContext {
            inputs,
            providers: CompilerQueryProviders::default(),
        })
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
let zeroes: [4]i32 = [0; 4];

fn main() i32 {
    zeroes[0]
}
"#,
            )]);
            let checked =
                CompilerDatabase::new(CompileRequest::new(loaded).with_optimization(level))
                    .check_program();
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
            dependency.from.name == "checked_program" && dependency.to.name == "checked_modules"
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
        assert!(!invalidated.contains(&"module_ast"), "{invalidated:?}");
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
                    dependency.from.name == "parse_ok_module_ids"
                        && dependency.to.name == "loaded_modules"
                })
        );
    }

    #[test]
    fn function_body_update_keeps_public_surface_cached() {
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

        assert!(!invalidated.contains(&"module_ast"), "{invalidated:?}");
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
        assert!(!invalidated.contains(&"public_surface"), "{invalidated:?}");

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
    }

    #[test]
    fn path_update_invalidates_diagnostics_without_recomputing_public_surface() {
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

        assert!(invalidated.contains(&"module_path"), "{invalidated:?}");
        assert!(invalidated.contains(&"checked_module"), "{invalidated:?}");
        assert!(!invalidated.contains(&"public_surface"), "{invalidated:?}");

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        assert_eq!(second.modules[0].path.as_str(), "renamed.nia");
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
        .check_program();

        assert!(checked.modules.is_empty());
    }

    #[test]
    fn missing_loaded_module_id_becomes_query_diagnostic() {
        fn unknown_parse_ok_module(_: &QueryDb<CompilerContext>) -> Vec<ModuleId> {
            vec![ModuleId(99)]
        }

        let providers = CompilerQueryProviders {
            parse_ok_module_ids: unknown_parse_ok_module,
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
        .check_program();

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
    fn program_signatures_query_uses_module_signature_queries() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "struct S { value: i32 }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(ProgramSignaturesQuery);
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "program_signatures" && dependency.to.name == "type_lowering"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "program_signatures" && dependency.to.name == "item_signatures"
        }));
    }

    #[test]
    fn public_surface_query_uses_module_defs_queries() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "pub struct S { value: i32 }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(PublicSurfaceQuery);
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "defs_by_module" && dependency.to.name == "module_defs"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "public_surface" && dependency.to.name == "defs_by_module"
        }));
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

        let _ = db.query(ExtensionMethodsQuery);
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_methods" && dependency.to.name == "module_defs"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_methods"
                && dependency.to.name == "active_module_item_tree"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_methods" && dependency.to.name == "module_ast"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_methods" && dependency.to.name == "type_lowering"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_methods"
                && dependency.to.name == "program_type_normalizations"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "program_type_normalizations"
                && dependency.to.name == "type_normalization"
        }));
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
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "flow_check" && dependency.to.name == "module_ast"
        }));
    }

    #[test]
    fn static_check_uses_full_active_item_tree_query() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "var global: i32 = 1; fn main() i32 { global }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(StaticCheckQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "static_check"
                && dependency.to.name == "full_active_module_item_tree"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "static_check" && dependency.to.name == "module_ast"
        }));
    }

    #[test]
    fn visible_extensions_use_program_type_normalizations_query() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "struct S { value: i32 } extend S { pub fn make(value: i32) S { { value: value } } }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(VisibleExtensionsQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "visible_extensions"
                && dependency.to.name == "program_type_normalizations"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "program_type_normalizations"
                && dependency.to.name == "type_normalization"
        }));
    }

    #[test]
    fn comptime_uses_program_context_map_queries() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "let VALUE = 1; fn main() i32 { VALUE }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(ComptimeQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "comptime" && dependency.to.name == "comptime_module"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "comptime" && dependency.to.name == "program_comptime_modules"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "comptime" && dependency.to.name == "program_defs_by_id"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "program_comptime_modules"
                && dependency.to.name == "comptime_module"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "program_defs_by_id" && dependency.to.name == "module_defs"
        }));
    }

    #[test]
    fn comptime_module_uses_full_active_item_tree_query() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "comptime fn value() usize { 1 } let VALUE = value();",
        )]);
        let db = query_db(loaded);

        let _ = db.query(ComptimeModuleQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "comptime_module"
                && dependency.to.name == "full_active_module_item_tree"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "comptime_module" && dependency.to.name == "module_ast"
        }));
    }

    #[test]
    fn semantic_use_table_query_combines_value_local_and_type_resolution() {
        let source = "let VALUE = 1; fn main() i32 { var local: i32 = VALUE; local }";
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
        let source = "fn main() i32 { var local: i32 = 1; local }";
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
    fn backend_lowering_uses_checked_module_body_ir() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "fn main() i32 { 0 }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(BackendLoweringQuery);
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering" && dependency.to.name == "checked_modules"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering"
                && dependency.to.name == "full_active_module_item_tree"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering" && dependency.to.name == "module_ast"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "checked_module" && dependency.to.name == "body_check"
        }));
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
    var total = 0;
    var iter = Counter { current: 0, end: 3 };
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
            .expect("root module should be executable-reachable");
        let next = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Method && def.name == "next").then_some(
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
    fn body_check_uses_comptime_semantic_modules_not_ast_module_map() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "comptime let N: usize = 4; fn main() i32 { var values: [N]i32 = [0; N]; values.len() as i32 }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(BodyCheckQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "comptime_module"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check"
                && dependency.to.name == "full_active_module_item_tree"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "program_comptime_modules"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "module_ast"
        }));
        assert!(
            !trace
                .dependencies
                .iter()
                .any(|dependency| dependency.to.name == "program_modules_by_id")
        );
    }

    #[test]
    fn invalidates_semantic_queries_after_public_surface_dependency_changes() {
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
        assert!(invalidated.contains(&"defs_by_module"), "{invalidated:?}");
        assert!(invalidated.contains(&"public_surface"), "{invalidated:?}");
        assert!(invalidated.contains(&"type_resolution"), "{invalidated:?}");

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
