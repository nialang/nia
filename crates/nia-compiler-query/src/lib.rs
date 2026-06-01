// SPDX-License-Identifier: GPL-3.0-or-later
mod program_signatures;
mod public_surface;
mod query;

use nia_abi_check::AbiCheck;
use nia_ast::Module;
use nia_backend_lower::BackendLowering;
use nia_body_check::BodyCheck;
use nia_comptime_check::ComptimeCheck;
use nia_defs::DefCollection;
use nia_diagnostic::Diagnostic;
use nia_flow_check::FlowCheck;
use nia_ids::ModuleId;
use nia_imports::{ImportAliasMap, ModuleGraph};
use nia_item_signatures::ItemSignatures;
use nia_layout::Layouts;
use nia_local_resolve::LocalResolution;
use nia_monomorphize::Monomorphization;
use nia_node_id::NodeOriginTable;
use nia_opt::OptimizationPolicy;
use nia_parser::ParseError;
use nia_source::{SourcePath, SourceVersion};
use nia_static_check::StaticCheck;
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_type_resolve::TypeResolution;
use nia_value_resolve::ValueResolution;

pub use query::check_loaded_program;
pub use query::check_loaded_program_with_options;

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedProgram {
    pub graph: ModuleGraph,
    pub imports: ImportAliasMap,
    pub modules: Vec<LoadedModule>,
    pub diagnostics: Vec<ProgramDiagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedModule {
    pub id: ModuleId,
    pub path: SourcePath,
    pub source_version: SourceVersion,
    pub source: String,
    pub module: Module,
    pub origins: NodeOriginTable,
    pub parse_errors: Vec<ParseError>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckedProgram {
    pub graph: ModuleGraph,
    pub imports: ImportAliasMap,
    pub optimization: OptimizationPolicy,
    pub modules: Vec<CheckedModule>,
    pub monomorphization: Monomorphization,
    pub backend_lowering: BackendLowering,
    pub diagnostics: Vec<ProgramDiagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramDiagnostic {
    pub path: SourcePath,
    pub diagnostic: Diagnostic,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckedModule {
    pub id: ModuleId,
    pub path: SourcePath,
    pub defs: DefCollection,
    pub type_resolution: TypeResolution,
    pub type_lowering: TypeLowering,
    pub value_resolution: ValueResolution,
    pub local_resolution: LocalResolution,
    pub item_signatures: ItemSignatures,
    pub type_normalization: TypeNormalization,
    pub comptime: ComptimeCheck,
    pub static_check: StaticCheck,
    pub layouts: Layouts,
    pub abi_check: AbiCheck,
    pub flow_check: FlowCheck,
    pub body_check: BodyCheck,
}

pub(crate) fn module_diagnostics(
    path: &SourcePath,
    diagnostics: &[Diagnostic],
) -> Vec<ProgramDiagnostic> {
    diagnostics
        .iter()
        .cloned()
        .map(|diagnostic| ProgramDiagnostic {
            path: path.clone(),
            diagnostic,
        })
        .collect()
}
