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

pub use nia_body_check::{
    ProviderDemand, ProviderFactRevision, ProviderFactRevisionTransition, ProviderRequest,
};

pub use nia_backend_lower::BackendOptimizationChange;
pub use nia_timing::TimingMode;
pub use query::{CompileRequest, CompilerDatabase};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActiveModuleItemTreeFactKind {
    Signature(nia_item_tree::SignatureItemSet),
    ConstSignature,
    Full,
}

pub trait LoaderFactProvider: Send + Sync {
    fn query_session(&self) -> Option<nia_query::QuerySession>;
    fn provider_fact_revision(&self) -> ProviderFactRevision;
    fn node_store(&self) -> nia_node_id::NodeStore;
    fn module_graph(&self) -> ModuleGraphSnapshot;
    fn loaded_module_source_identities(&self) -> Vec<SourceIdentity>;
    fn module_path(&self, module_id: ModuleId) -> Option<SourcePath>;
    fn module_source_version(&self, module_id: ModuleId) -> Option<SourceVersion>;
    fn module_provider_summary(&self, module_id: ModuleId) -> Option<ProviderSummary>;
    fn module_origins(&self, module_id: ModuleId) -> Option<NodeOriginTable>;
    fn module_parse_errors(&self, module_id: ModuleId) -> Option<Vec<ParseError>>;
    fn module_item_tree(&self, module_id: ModuleId) -> Option<ModuleItemTree>;
    fn active_module_item_tree(
        &self,
        module_id: ModuleId,
        kind: ActiveModuleItemTreeFactKind,
    ) -> Option<ActiveModuleItemTree>;
    fn load_diagnostics(&self) -> Vec<ProgramDiagnostic>;
    fn symbols(&self) -> SymbolTable;
    fn target(&self) -> TargetConfig;
    fn runtime(&self) -> RuntimeModel;
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedProgram {
    pub graph: ModuleGraphSnapshot,
    pub provider_fact_revision: ProviderFactRevision,
    pub symbols: SymbolTable,
    pub target: TargetConfig,
    pub runtime: RuntimeModel,
    pub modules: Vec<LoadedModule>,
    pub diagnostics: Vec<ProgramDiagnostic>,
}

impl LoaderFactProvider for LoadedProgram {
    fn query_session(&self) -> Option<nia_query::QuerySession> {
        None
    }

    fn provider_fact_revision(&self) -> ProviderFactRevision {
        self.provider_fact_revision
    }

    fn node_store(&self) -> nia_node_id::NodeStore {
        self.modules
            .first()
            .map(|module| module.origins.node_store().clone())
            .unwrap_or_default()
    }

    fn module_graph(&self) -> ModuleGraphSnapshot {
        self.graph.clone()
    }

    fn loaded_module_source_identities(&self) -> Vec<SourceIdentity> {
        self.modules
            .iter()
            .map(|module| module.source_identity.clone())
            .collect()
    }

    fn module_path(&self, module_id: ModuleId) -> Option<SourcePath> {
        self.modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.path.clone())
    }

    fn module_source_version(&self, module_id: ModuleId) -> Option<SourceVersion> {
        self.modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.source_version)
    }

    fn module_provider_summary(&self, module_id: ModuleId) -> Option<ProviderSummary> {
        self.modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.provider_summary.clone())
    }

    fn module_origins(&self, module_id: ModuleId) -> Option<NodeOriginTable> {
        self.modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.origins.clone())
    }

    fn module_parse_errors(&self, module_id: ModuleId) -> Option<Vec<ParseError>> {
        self.modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.parse_errors.clone())
    }

    fn module_item_tree(&self, module_id: ModuleId) -> Option<ModuleItemTree> {
        self.modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.item_tree.clone())
    }

    fn active_module_item_tree(
        &self,
        module_id: ModuleId,
        kind: ActiveModuleItemTreeFactKind,
    ) -> Option<ActiveModuleItemTree> {
        let tree = &self
            .modules
            .iter()
            .find(|module| module.id == module_id)?
            .active_item_tree;
        Some(match kind {
            ActiveModuleItemTreeFactKind::Signature(set) => tree.signature_items(set),
            ActiveModuleItemTreeFactKind::ConstSignature => tree.const_signature_items(),
            ActiveModuleItemTreeFactKind::Full => tree.clone(),
        })
    }

    fn load_diagnostics(&self) -> Vec<ProgramDiagnostic> {
        self.diagnostics.clone()
    }

    fn symbols(&self) -> SymbolTable {
        self.symbols.clone()
    }

    fn target(&self) -> TargetConfig {
        self.target.clone()
    }

    fn runtime(&self) -> RuntimeModel {
        self.runtime
    }
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
