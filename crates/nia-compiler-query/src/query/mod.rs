// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    CheckedModule, CheckedProgram, LoadedModule, LoadedProgram, ProgramDiagnostic,
    module_diagnostics,
    program_signatures::{
        ExtensionModuleInput, ModuleSignatureInput, VisibleExtensionsForModule,
        collect_extension_methods, collect_program_comptimes, collect_program_enums,
        collect_program_functions, collect_program_globals, collect_program_structs,
        collect_program_unions, visible_extensions_for_module,
    },
    public_surface::compute_public_surfaces,
};
use nia_backend_lower::BackendLowerModuleInput;
use nia_body_check::{
    ProgramComptimeSignature, ProgramEnumSignature, ProgramFunctionSignature,
    ProgramGlobalSignature, ProgramSignatureMaps, ProgramStructSignature, ProgramUnionSignature,
};
use nia_comptime_check::ComptimeCheck;
use nia_defs::{DefCollection, ModuleUsingScope, PublicSurfaces};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, ModuleId};
use nia_imports::{ImportAliasMap, ModuleGraph};
use nia_item_signatures::ItemSignatures;
use nia_local_resolve::LocalResolution;
use nia_monomorphize::MonomorphizeModuleInput;
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
    check_loaded_program_with_providers(loaded, CompilerQueryProviders::default())
}

fn check_loaded_program_with_providers(
    loaded: LoadedProgram,
    providers: CompilerQueryProviders,
) -> CheckedProgram {
    let graph = loaded.graph.clone();
    let imports = loaded.imports.clone();
    let db = QueryDb::new(DriverContext { loaded, providers });
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        db.try_query(CheckedProgramQuery)
    })) {
        Ok(Ok(checked)) => checked,
        Ok(Err(err)) => checked_program_from_query_error(graph, imports, err),
        Err(payload) => match payload.downcast::<QueryError>() {
            Ok(err) => checked_program_from_query_error(graph, imports, *err),
            Err(payload) => std::panic::resume_unwind(payload),
        },
    }
}

fn checked_program_from_query_error(
    graph: ModuleGraph,
    imports: ImportAliasMap,
    err: QueryError,
) -> CheckedProgram {
    CheckedProgram {
        graph,
        imports,
        modules: Vec::new(),
        monomorphization: nia_monomorphize::Monomorphization {
            instances: Vec::new(),
            diagnostics: Vec::new(),
        },
        backend_lowering: nia_backend_lower::BackendLowering {
            program: nia_backend_ir::BackendProgram {
                modules: Vec::new(),
            },
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
    }
}

struct DriverContext {
    loaded: LoadedProgram,
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
            source: source.to_string(),
            module,
            parse_errors,
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
            providers,
        );

        assert!(checked.modules.is_empty());
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
}
