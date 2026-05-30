// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    CheckedModule, CheckedProgram, LoadedModule, LoadedProgram, ProgramDiagnostic,
    module_diagnostics,
    program_signatures::{
        VisibleExtensionsForModule, collect_extension_methods, collect_program_comptimes,
        collect_program_enums, collect_program_functions, collect_program_globals,
        collect_program_structs, collect_program_unions, visible_extensions_for_module,
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
use nia_imports::{ImportAliasMap, ModuleGraph, SourcePath};
use nia_item_signatures::ItemSignatures;
use nia_local_resolve::LocalResolution;
use nia_monomorphize::MonomorphizeModuleInput;
use nia_query::{QueryDb, QueryKey};
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
    let db = QueryDb::new(DriverContext { loaded, providers });
    db.query(CheckedProgramQuery)
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
            LoadedProgram {
                graph: ModuleGraph::new(SourcePath::new("main.nia")),
                imports: ImportAliasMap::default(),
                modules: vec![LoadedModule {
                    id: ModuleId(0),
                    path: SourcePath::new("main.nia"),
                    source: "fn main() i32 { 0 }".to_string(),
                    module: nia_ast::Module { items: Vec::new() },
                    parse_errors: Vec::new(),
                }],
                diagnostics: Vec::new(),
            },
            providers,
        );

        assert!(checked.modules.is_empty());
    }
}
