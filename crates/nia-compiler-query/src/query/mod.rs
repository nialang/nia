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
mod diagnostics;
mod module;
mod program;

use base::*;
use diagnostics::*;
use module::*;
use program::*;

pub fn check_loaded_program(loaded: LoadedProgram) -> CheckedProgram {
    let db = QueryDb::new(DriverContext { loaded });
    db.query(CheckedProgramQuery)
}

struct DriverContext {
    loaded: LoadedProgram,
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
