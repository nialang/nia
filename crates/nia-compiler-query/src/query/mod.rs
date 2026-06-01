// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    CheckedModule, CheckedProgram, LoadedModule, LoadedProgram, ProgramDiagnostic,
    module_diagnostics,
    program_signatures::{
        ExtensionModuleInput, ModuleSignatureInput, VisibleExtensionsForModule,
        collect_extension_methods, collect_program_comptimes, collect_program_enums,
        collect_program_functions, collect_program_globals, collect_program_structs,
        collect_program_traits, collect_program_unions, visible_extensions_for_module,
    },
    public_surface::compute_public_surfaces,
};
use nia_backend_lower::BackendLowerModuleInput;
use nia_comptime_check::ComptimeCheck;
use nia_defs::{DefCollection, ModuleUsingScope, PublicSurfaces};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, ModuleId};
use nia_imports::{ImportAliasMap, ModuleGraph};
use nia_item_signatures::{
    ItemSignatures, ProgramComptimeSignature, ProgramEnumSignature, ProgramFunctionSignature,
    ProgramGlobalSignature, ProgramSignatureMaps, ProgramStructSignature, ProgramTraitSignature,
    ProgramUnionSignature,
};
use nia_local_resolve::LocalResolution;
use nia_monomorphize::MonomorphizeModuleInput;
use nia_opt::{OptimizationLevel, OptimizationPolicy};
use nia_query::{QueryDb, QueryError, QueryKey};
use nia_source::SourcePath;
use nia_span::Span;
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_type_resolve::TypeResolution;
use nia_value_resolve::ValueResolution;

mod base;
mod checked;
mod checks;
mod diagnostics;
mod program;
mod providers;
mod resolve;
mod types;

use base::*;
use checked::*;
use checks::*;
use diagnostics::*;
use program::*;
use providers::*;
use resolve::*;
use types::*;

pub fn check_loaded_program(loaded: LoadedProgram) -> CheckedProgram {
    check_loaded_program_with_options(loaded, OptimizationLevel::default())
}

pub fn check_loaded_program_with_options(
    loaded: LoadedProgram,
    optimization: OptimizationLevel,
) -> CheckedProgram {
    check_loaded_program_with_providers(
        loaded,
        optimization.policy(),
        CompilerQueryProviders::default(),
    )
}

fn check_loaded_program_with_providers(
    loaded: LoadedProgram,
    optimization: OptimizationPolicy,
    providers: CompilerQueryProviders,
) -> CheckedProgram {
    let graph = loaded.graph.clone();
    let imports = loaded.imports.clone();
    let db = QueryDb::new(DriverContext {
        loaded,
        optimization,
        providers,
    });
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        db.try_query(CheckedProgramQuery)
    })) {
        Ok(Ok(checked)) => checked,
        Ok(Err(err)) => checked_program_from_query_error(graph, imports, optimization, err),
        Err(payload) => match payload.downcast::<QueryError>() {
            Ok(err) => checked_program_from_query_error(graph, imports, optimization, *err),
            Err(payload) => std::panic::resume_unwind(payload),
        },
    }
}

fn checked_program_from_query_error(
    graph: ModuleGraph,
    imports: ImportAliasMap,
    optimization: OptimizationPolicy,
    err: QueryError,
) -> CheckedProgram {
    CheckedProgram {
        graph,
        imports,
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
            Diagnostic::error(Span::default(), message)
        }
        QueryError::InvalidInput { query, message } => Diagnostic::error(
            Span::default(),
            format!("invalid query input for {}: {message}", query.description),
        ),
    }
}

struct DriverContext {
    loaded: LoadedProgram,
    optimization: OptimizationPolicy,
    providers: CompilerQueryProviders,
}

impl DriverContext {
    fn loaded_module(&self, module_id: ModuleId) -> Option<&LoadedModule> {
        self.loaded
            .modules
            .iter()
            .find(|module| module.id == module_id)
    }

    fn path_for_module(&self, module_id: ModuleId) -> SourcePath {
        self.loaded_module(module_id)
            .map(|module| module.path.clone())
            .unwrap_or_else(|| SourcePath::new("<unknown>"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_source::{SourceId, SourceRevision};

    fn loaded_program_with_modules(modules: Vec<LoadedModule>) -> LoadedProgram {
        LoadedProgram {
            graph: ModuleGraph::new(SourcePath::new("main.nia")),
            imports: ImportAliasMap::default(),
            modules,
            diagnostics: Vec::new(),
        }
    }

    fn loaded_module(id: ModuleId, path: &str, source: &str) -> LoadedModule {
        let (module, parse_errors) = nia_parser::parse_module(source);
        assert!(parse_errors.is_empty(), "{parse_errors:?}");
        LoadedModule {
            id,
            path: SourcePath::new(path),
            source_version: nia_source::SourceVersion {
                id: SourceId(id.0),
                revision: SourceRevision::INITIAL,
            },
            source: source.to_string(),
            module,
            parse_errors,
            origins: nia_node_id::NodeOriginTable::default(),
        }
    }

    #[test]
    fn compiler_query_providers_can_override_query_execution() {
        fn no_parse_ok_modules(_: &QueryDb<DriverContext>) -> Vec<ModuleId> {
            Vec::new()
        }

        let providers = CompilerQueryProviders {
            parse_ok_module_ids: no_parse_ok_modules,
            ..CompilerQueryProviders::default()
        };
        let checked = check_loaded_program_with_providers(
            loaded_program_with_modules(vec![loaded_module(
                ModuleId(0),
                "main.nia",
                "fn main() i32 { 0 }",
            )]),
            OptimizationPolicy::default(),
            providers,
        );

        assert!(checked.modules.is_empty());
    }

    #[test]
    fn missing_loaded_module_id_becomes_query_diagnostic() {
        fn unknown_parse_ok_module(_: &QueryDb<DriverContext>) -> Vec<ModuleId> {
            vec![ModuleId(99)]
        }

        let providers = CompilerQueryProviders {
            parse_ok_module_ids: unknown_parse_ok_module,
            ..CompilerQueryProviders::default()
        };
        let checked = check_loaded_program_with_providers(
            loaded_program_with_modules(vec![loaded_module(
                ModuleId(0),
                "main.nia",
                "fn main() i32 { 0 }",
            )]),
            OptimizationPolicy::default(),
            providers,
        );

        assert!(checked.modules.is_empty());
        assert_eq!(checked.diagnostics.len(), 1);
        assert!(
            checked.diagnostics[0]
                .diagnostic
                .message
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
        let db = QueryDb::new(DriverContext {
            loaded,
            optimization: OptimizationPolicy::default(),
            providers: CompilerQueryProviders::default(),
        });

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
        let db = QueryDb::new(DriverContext {
            loaded,
            optimization: OptimizationPolicy::default(),
            providers: CompilerQueryProviders::default(),
        });

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
    fn extension_queries_use_module_semantic_queries() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "struct S { value: i32 } extend S { pub fn make(value: i32) S { { value: value } } }",
        )]);
        let db = QueryDb::new(DriverContext {
            loaded,
            optimization: OptimizationPolicy::default(),
            providers: CompilerQueryProviders::default(),
        });

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
                && dependency.to.name == "type_normalization"
        }));
    }

    #[test]
    fn backend_lowering_uses_function_body_query() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "fn main() i32 { 0 }",
        )]);
        let db = QueryDb::new(DriverContext {
            loaded,
            optimization: OptimizationPolicy::default(),
            providers: CompilerQueryProviders::default(),
        });

        let _ = db.query(BackendLoweringQuery);
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering" && dependency.to.name == "function_bodies"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "function_bodies" && dependency.to.name == "body_check"
        }));
    }

    #[test]
    fn invalidates_semantic_queries_after_public_surface_dependency_changes() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "pub struct S { value: i32 } fn main() i32 { 0 }",
        )]);
        let db = QueryDb::new(DriverContext {
            loaded,
            optimization: OptimizationPolicy::default(),
            providers: CompilerQueryProviders::default(),
        });

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
}
