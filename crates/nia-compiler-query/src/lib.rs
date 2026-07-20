// SPDX-License-Identifier: GPL-3.0-or-later
mod query;

use nia_abi_check::AbiCheck;
use nia_backend_lower::BackendLowering;
use nia_body_ir::BodyIr;
use nia_const_check::ConstCheck;
use nia_defs::DefCollection;
use nia_diagnostic::{Diagnostic, Severity};
use nia_flow_check::FlowCheck;
use nia_ids::{GlobalDefId, ModuleId};
use nia_imports::ModuleGraphSnapshot;
use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
use nia_layout::Layouts;
use nia_local_resolve::LocalResolution;
use nia_monomorphize::Monomorphization;
use nia_node_id::NodeOriginTable;
use nia_opt::OptimizationPolicy;
use nia_parser::ParseError;
use nia_provider_summary::ProviderSummary;
use nia_sema_ir::{SemanticFacts, SemanticUseTable};
use nia_source::{SourceIdentity, SourcePath, SourceVersion};
use nia_static_check::StaticCheck;
use nia_symbol_table::SymbolTable;
use nia_target_config::TargetConfig;
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_type_resolve::TypeResolution;
use nia_value_resolve::ValueResolution;

pub use nia_body_check::{ProviderDemand, ProviderRequest};

pub use nia_backend_lower::BackendOptimizationChange;
pub use nia_timing::TimingMode;
pub use query::{CompileRequest, CompilerDatabase};

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedProgram {
    pub graph: ModuleGraphSnapshot,
    pub symbols: SymbolTable,
    pub target: TargetConfig,
    pub runtime: RuntimeModel,
    pub modules: Vec<LoadedModule>,
    pub diagnostics: Vec<ProgramDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RuntimeModel {
    #[default]
    Bare,
    FreestandingExecutable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedModule {
    pub id: ModuleId,
    pub path: SourcePath,
    pub source_identity: SourceIdentity,
    pub source_version: SourceVersion,
    pub item_tree: ModuleItemTree,
    pub active_item_tree: ActiveModuleItemTree,
    pub provider_summary: ProviderSummary,
    pub origins: NodeOriginTable,
    pub parse_errors: Vec<ParseError>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckedProgram {
    pub graph: ModuleGraphSnapshot,
    pub optimization: OptimizationPolicy,
    pub modules: Vec<std::sync::Arc<CheckedModule>>,
    pub diagnostics: Vec<ProgramDiagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodegenProgram {
    pub type_store: std::sync::Arc<nia_ty::TypeStore>,
    pub graph: ModuleGraphSnapshot,
    pub optimization: OptimizationPolicy,
    pub modules: Vec<std::sync::Arc<CheckedModule>>,
    pub monomorphization: std::sync::Arc<Monomorphization>,
    pub backend_lowering: std::sync::Arc<BackendLowering>,
    pub diagnostics: Vec<ProgramDiagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramDiagnostic {
    pub path: SourcePath,
    pub diagnostic: Diagnostic,
}

impl ProgramDiagnostic {
    pub fn is_error(&self) -> bool {
        self.diagnostic.severity == Severity::Error
    }

    pub fn is_warning(&self) -> bool {
        self.diagnostic.severity == Severity::Warning
    }
}

pub fn has_error_diagnostics(diagnostics: &[ProgramDiagnostic]) -> bool {
    diagnostics.iter().any(ProgramDiagnostic::is_error)
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckedModule {
    pub id: ModuleId,
    pub path: SourcePath,
    pub defs: std::sync::Arc<DefCollection>,
    pub type_resolution: std::sync::Arc<TypeResolution>,
    pub type_lowering: std::sync::Arc<TypeLowering>,
    pub value_resolution: std::sync::Arc<ValueResolution>,
    pub local_resolution: std::sync::Arc<LocalResolution>,
    pub type_normalization: std::sync::Arc<TypeNormalization>,
    pub const_eval: std::sync::Arc<ConstCheck>,
    pub static_check: std::sync::Arc<StaticCheck>,
    pub layouts: std::sync::Arc<Layouts>,
    pub abi_check: std::sync::Arc<AbiCheck>,
    pub flow_check: std::sync::Arc<FlowCheck>,
    pub body_ir: std::sync::Arc<BodyIr>,
    pub semantic_uses: std::sync::Arc<SemanticUseTable>,
    pub semantic_facts: std::sync::Arc<SemanticFacts>,
    pub provider_demands: std::sync::Arc<std::collections::HashSet<ProviderDemand>>,
    pub executable_reachable_globals: Option<std::collections::HashSet<GlobalDefId>>,
    pub executable_reachable_structs:
        Option<std::sync::Arc<std::collections::HashSet<GlobalDefId>>>,
    pub executable_reachable_unions: Option<std::sync::Arc<std::collections::HashSet<GlobalDefId>>>,
    pub executable_type_only: bool,
    pub body_diagnostics: std::sync::Arc<Vec<Diagnostic>>,
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
