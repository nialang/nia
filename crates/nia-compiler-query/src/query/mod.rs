// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    CheckedModule, CheckedProgram, LoadedModule, LoadedProgram, ProgramDiagnostic, TimingMode,
    module_diagnostics,
    program_signatures::{
        ExtensionModuleInput, ModuleSignatureInput, VisibleExtensionsForModule,
        VisibleExtensionsInput, collect_extension_associated_values, collect_extension_methods,
        collect_program_comptimes, collect_program_enums, collect_program_functions,
        collect_program_globals, collect_program_structs, collect_program_traits,
        collect_program_unions, visible_extensions_for_module,
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
use nia_opt::{NiaOptimizationLevel, OptimizationPolicy};
use nia_query::{QueryDb, QueryError, QueryKey, QueryTrace};
use nia_source::SourcePath;
use nia_span::Span;
use nia_target_config::TargetConfig;
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_type_resolve::TypeResolution;
use nia_value_resolve::ValueResolution;
use std::{collections::HashMap, sync::Arc};

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
    graph: ModuleGraph,
    optimization: OptimizationPolicy,
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
            Ok(Err(err)) => {
                checked_program_from_query_error(self.graph.clone(), self.optimization, err)
            }
            Err(payload) => match payload.downcast::<QueryError>() {
                Ok(err) => {
                    checked_program_from_query_error(self.graph.clone(), self.optimization, *err)
                }
                Err(payload) => std::panic::resume_unwind(payload),
            },
        }
    }

    pub fn query_trace(&self) -> QueryTrace {
        self.db.query_trace()
    }
}

impl std::fmt::Debug for CompilerDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompilerDatabase")
            .field("graph", &self.graph)
            .field("optimization", &self.optimization)
            .finish_non_exhaustive()
    }
}

fn compiler_database_with_providers(
    request: CompileRequest,
    providers: CompilerQueryProviders,
) -> CompilerDatabase {
    let loaded = request.loaded;
    let graph = loaded.graph.clone();
    let target = loaded.target.clone();
    let modules_by_id = index_loaded_modules(&loaded);
    let optimization = request.optimization.policy();
    let db = QueryDb::new(CompilerContext {
        loaded,
        modules_by_id,
        target,
        optimization,
        timings: request.timings,
        providers,
    });
    CompilerDatabase {
        db,
        graph,
        optimization,
    }
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
    loaded: LoadedProgram,
    modules_by_id: HashMap<ModuleId, usize>,
    target: TargetConfig,
    optimization: OptimizationPolicy,
    timings: TimingMode,
    providers: CompilerQueryProviders,
}

impl CompilerContext {
    fn loaded_module(&self, module_id: ModuleId) -> Option<&LoadedModule> {
        self.modules_by_id
            .get(&module_id)
            .and_then(|index| self.loaded.modules.get(*index))
    }

    fn path_for_module(&self, module_id: ModuleId) -> SourcePath {
        self.loaded_module(module_id)
            .map(|module| module.path.clone())
            .unwrap_or_else(|| SourcePath::new("<unknown>"))
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
        let (module, parse_errors) = nia_parser::parse_module(source);
        assert!(parse_errors.is_empty(), "{parse_errors:?}");
        let item_tree = ModuleItemTree::from_module(&module);
        LoadedModule {
            id,
            path: SourcePath::new(path),
            source_version: nia_source::SourceVersion {
                id: SourceId(id.0),
                revision: SourceRevision::INITIAL,
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
        let target = loaded.target.clone();
        let modules_by_id = index_loaded_modules(&loaded);
        QueryDb::new(CompilerContext {
            loaded,
            modules_by_id,
            target,
            optimization: OptimizationPolicy::default(),
            timings: TimingMode::Off,
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
            dependency.from.name == "module_item_tree" && dependency.to.name == "loaded_module"
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
            dependency.from.name == "body_check" && dependency.to.name == "program_comptime_modules"
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
        let invalidation = db.invalidate(ModuleItemTreeQuery(ModuleId(0)));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(invalidated.contains(&"module_item_tree"), "{invalidated:?}");
        assert!(
            invalidated.contains(&"active_module_item_tree"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"module_defs"), "{invalidated:?}");
    }
}
