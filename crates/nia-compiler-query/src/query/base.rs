// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_node_id::NodeOriginTable;
use nia_parser::ParseError;
use std::collections::HashMap;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CheckedProgramQuery;

impl QueryKey<CompilerContext> for CheckedProgramQuery {
    type Value = CheckedProgramAnalysis;

    fn name() -> &'static str {
        "checked_program"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.checked_program)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct EntryCheckedProgramQuery;

impl QueryKey<CompilerContext> for EntryCheckedProgramQuery {
    type Value = CheckedProgramAnalysis;

    fn name() -> &'static str {
        "entry_checked_program"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.entry_checked_program)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExecutableProviderDemandsQuery;

impl QueryKey<CompilerContext> for ExecutableProviderDemandsQuery {
    type Value = Vec<crate::ProviderDemand>;

    fn name() -> &'static str {
        "executable_provider_demands"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        provide_executable_provider_demands(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CodegenPreparationQuery;

impl QueryKey<CompilerContext> for CodegenPreparationQuery {
    type Value = CodegenPreparation;

    fn name() -> &'static str {
        "codegen_preparation"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        provide_codegen_preparation(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CodegenProgramQuery;

impl QueryKey<CompilerContext> for CodegenProgramQuery {
    type Value = CodegenProgram;

    fn name() -> &'static str {
        "codegen_program"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.codegen_program)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleGraphQuery;

impl QueryKey<CompilerContext> for ModuleGraphQuery {
    type Value = nia_imports::ModuleGraphSnapshot;

    fn name() -> &'static str {
        "module_graph"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.context().loader_facts().module_graph()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleGraphEntryQuery;

impl QueryKey<CompilerContext> for ModuleGraphEntryQuery {
    type Value = StableModuleKey;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

    fn name() -> &'static str {
        "module_graph_entry"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        let graph = db.get(ModuleGraphQuery);
        graph
            .stable_key(graph.entry())
            .cloned()
            .expect("compiler entry must have a stable module key")
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
        Some(stable_module_key_fingerprint(
            "nia.compiler.module-graph-entry.v1",
            value,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleGraphPathQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ModuleGraphPathQuery {
    type Value = Option<nia_imports::ModulePath>;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

    fn name() -> &'static str {
        "module_graph_path"
    }

    fn description(&self) -> String {
        format!("module_graph_path({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.get(ModuleGraphQuery)
            .get(self.0)
            .map(|module| module.module_path.clone())
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
        Some(module_graph_path_fingerprint(value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleGraphParentQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ModuleGraphParentQuery {
    type Value = Option<StableModuleKey>;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

    fn name() -> &'static str {
        "module_graph_parent"
    }

    fn description(&self) -> String {
        format!("module_graph_parent({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        let graph = db.get(ModuleGraphQuery);
        let parent = graph.get(self.0)?.parent?;
        graph.stable_key(parent).cloned()
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
        Some(optional_stable_module_key_fingerprint(
            "nia.compiler.module-graph-parent.v1",
            value.as_ref(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleGraphChildQuery(pub(super) ModuleId, pub(super) SymbolId);

impl QueryKey<CompilerContext> for ModuleGraphChildQuery {
    type Value = Option<(StableModuleKey, nia_ids::Visibility)>;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

    fn name() -> &'static str {
        "module_graph_child"
    }

    fn description(&self) -> String {
        format!("module_graph_child({:?}, {:?})", self.0, self.1)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        let graph = db.get(ModuleGraphQuery);
        let module = graph.get(self.0)?;
        let target = module.children.get(&self.1).copied()?;
        let declaration = module
            .declarations
            .iter()
            .find(|declaration| declaration.name == self.1 && declaration.target == target)?;
        Some((graph.stable_key(target)?.clone(), declaration.visibility))
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
        Some(module_graph_child_fingerprint(value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModulePackageRootQuery(pub(super) SymbolId);

impl QueryKey<CompilerContext> for ModulePackageRootQuery {
    type Value = Option<StableModuleKey>;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

    fn name() -> &'static str {
        "module_package_root"
    }

    fn description(&self) -> String {
        format!("module_package_root({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        let graph = db.get(ModuleGraphQuery);
        let root = graph.package_root(&self.0)?;
        graph.stable_key(root).cloned()
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
        Some(optional_stable_module_key_fingerprint(
            "nia.compiler.module-package-root.v1",
            value.as_ref(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct LoadedModulesQuery;

impl QueryKey<CompilerContext> for LoadedModulesQuery {
    type Value = StableModuleSequence;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

    fn name() -> &'static str {
        "loaded_modules"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        StableModuleSequence::from_source_identities(
            db.context()
                .loader_facts()
                .loaded_module_source_identities(),
        )
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
        Some(stable_module_sequence_fingerprint(
            "nia.compiler.loaded-modules.v1",
            value,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramLoadDiagnosticsQuery;

impl QueryKey<CompilerContext> for ProgramLoadDiagnosticsQuery {
    type Value = Vec<ProgramDiagnostic>;

    fn name() -> &'static str {
        "program_load_diagnostics"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.context().loader_facts().load_diagnostics()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CompilerTargetQuery;

impl QueryKey<CompilerContext> for CompilerTargetQuery {
    type Value = TargetConfig;

    fn name() -> &'static str {
        "compiler_target"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.context().loader_facts().target()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CompilerRuntimeQuery;

impl QueryKey<CompilerContext> for CompilerRuntimeQuery {
    type Value = RuntimeModel;

    fn name() -> &'static str {
        "compiler_runtime"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.context().loader_facts().runtime()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProviderFactRevisionQuery;

impl QueryKey<CompilerContext> for ProviderFactRevisionQuery {
    type Value = crate::ProviderFactRevision;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

    fn name() -> &'static str {
        "provider_fact_revision"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.get(ProviderFactWorklistQuery).revision()
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
        Some(provider_fact_revision_fingerprint(*value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProviderFactWorklistQuery;

impl QueryKey<CompilerContext> for ProviderFactWorklistQuery {
    type Value = crate::ProviderFactSnapshot;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

    fn name() -> &'static str {
        "provider_fact_worklist"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.context().provider_fact_worklist()
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
        Some(provider_fact_worklist_fingerprint(value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct BodyActivationWorklistQuery;

impl QueryKey<CompilerContext> for BodyActivationWorklistQuery {
    type Value = BodyActivationWorklist;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "body_activation_worklist"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        let graph = db.get(ModuleGraphQuery);
        let modules = graph
            .modules()
            .filter(|module| module.process_used_paths)
            .map(|module| {
                let stable_key = graph.stable_key(module.id).unwrap_or_else(|| {
                    panic!(
                        "Nia ICE: missing stable key for activated module {:?}",
                        module.id
                    )
                });
                (stable_key.clone(), module.id)
            })
            .collect();
        BodyActivationWorklist {
            modules: Arc::new(modules),
        }
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExecutableFactEpochQuery;

impl QueryKey<CompilerContext> for ExecutableFactEpochQuery {
    type Value = ExecutableFactEpoch;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "executable_fact_epoch"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        let graph = db.get(ModuleGraphQuery);
        let modules = graph
            .modules()
            .map(|module| (module.id, *db.get(ModuleSourceVersionQuery(module.id))))
            .collect();
        let runtime_root_modules = graph
            .modules()
            .filter(|module| graph.is_executable_root_module(module.id))
            .map(|module| module.id)
            .collect();
        ExecutableFactEpoch {
            entry_module: graph.entry(),
            runtime_root_modules,
            modules,
            target: db.get(CompilerTargetQuery).as_ref().clone(),
            runtime: *db.get(CompilerRuntimeQuery),
        }
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExecutableRootModulesQuery;

impl QueryKey<CompilerContext> for ExecutableRootModulesQuery {
    type Value = (ModuleId, Vec<ModuleId>);

    fn name() -> &'static str {
        "executable_root_modules"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        let graph = db.get(ModuleGraphQuery);
        let runtime_root_modules = graph
            .modules()
            .filter(|module| graph.is_executable_root_module(module.id))
            .map(|module| module.id)
            .collect();
        (graph.entry(), runtime_root_modules)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CompilerOptimizationQuery;

impl QueryKey<CompilerContext> for CompilerOptimizationQuery {
    type Value = OptimizationPolicy;

    fn name() -> &'static str {
        "compiler_optimization"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.context().optimization()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ParseOkModuleIdsQuery;

impl QueryKey<CompilerContext> for ParseOkModuleIdsQuery {
    type Value = StableModuleSequence;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

    fn name() -> &'static str {
        "parse_ok_module_ids"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.parse_ok_module_ids)(db)
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
        Some(stable_module_sequence_fingerprint(
            "nia.compiler.parse-ok-module-ids.v1",
            value,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SemanticModuleIdsQuery;

impl QueryKey<CompilerContext> for SemanticModuleIdsQuery {
    type Value = StableModuleSequence;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

    fn name() -> &'static str {
        "semantic_module_ids"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.semantic_module_ids)(db)
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
        Some(stable_module_sequence_fingerprint(
            "nia.compiler.semantic-module-ids.v1",
            value,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModulePathQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ModulePathQuery {
    type Value = SourcePath;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

    fn name() -> &'static str {
        "module_path"
    }

    fn description(&self) -> String {
        format!("module_path({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        db.context().module_path(db, self.0)
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
        Some(source_path_fingerprint(
            "nia.compiler.module-source-path.v1",
            value,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleSourceVersionQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ModuleSourceVersionQuery {
    type Value = SourceVersion;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

    fn name() -> &'static str {
        "module_source_version"
    }

    fn description(&self) -> String {
        format!("module_source_version({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        db.context().module_source_version(db, self.0)
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
        Some(source_version_fingerprint(
            "nia.compiler.module-source-version.v1",
            *value,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleOriginsQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ModuleOriginsQuery {
    type Value = NodeOriginTable;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "module_origins"
    }

    fn description(&self) -> String {
        format!("module_origins({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        db.context().module_origins(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleParseErrorsQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ModuleParseErrorsQuery {
    type Value = Vec<ParseError>;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "module_parse_errors"
    }

    fn description(&self) -> String {
        format!("module_parse_errors({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        db.context().module_parse_errors(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleItemTreeInputQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ModuleItemTreeInputQuery {
    type Value = ModuleItemTree;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "module_item_tree_input"
    }

    fn description(&self) -> String {
        format!("module_item_tree_input({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        db.context().module_item_tree(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct DeclarationModuleItemTreeInputQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for DeclarationModuleItemTreeInputQuery {
    type Value = ModuleItemTree;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "declaration_module_item_tree_input"
    }

    fn description(&self) -> String {
        format!("declaration_module_item_tree_input({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        db.context().declaration_module_item_tree(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ActiveModuleItemTreeInputQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ActiveModuleItemTreeInputQuery {
    type Value = ActiveModuleItemTree;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "active_module_item_tree_input"
    }

    fn description(&self) -> String {
        format!("active_module_item_tree_input({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        db.context().active_module_item_tree(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct DeclarationActiveModuleItemTreeInputQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for DeclarationActiveModuleItemTreeInputQuery {
    type Value = ActiveModuleItemTree;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "declaration_active_module_item_tree_input"
    }

    fn description(&self) -> String {
        format!("declaration_active_module_item_tree_input({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        db.context().declaration_active_module_item_tree(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct FullModuleItemTreeInputQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for FullModuleItemTreeInputQuery {
    type Value = ModuleItemTree;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "full_module_item_tree_input"
    }

    fn description(&self) -> String {
        format!("full_module_item_tree_input({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        db.context().full_module_item_tree(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct FullActiveModuleItemTreeInputQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for FullActiveModuleItemTreeInputQuery {
    type Value = ActiveModuleItemTree;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "full_active_module_item_tree_input"
    }

    fn description(&self) -> String {
        format!("full_active_module_item_tree_input({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        db.context().full_active_module_item_tree(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleDefsQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct FullModuleDefsQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct PublicSurfaceModuleFactsQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleItemTreeQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ActiveModuleItemTreeQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct DeclarationModuleItemTreeQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct DeclarationActiveModuleItemTreeQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct FullModuleItemTreeQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct FullActiveModuleItemTreeQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ModuleItemTreeQuery {
    type Value = ModuleItemTree;

    fn name() -> &'static str {
        "module_item_tree"
    }

    fn description(&self) -> String {
        format!("module_item_tree({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.module_item_tree)(db, self.0)
    }
}

impl QueryKey<CompilerContext> for FullModuleItemTreeQuery {
    type Value = ModuleItemTree;

    fn name() -> &'static str {
        "full_module_item_tree"
    }

    fn description(&self) -> String {
        format!("full_module_item_tree({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.full_module_item_tree)(db, self.0)
    }
}

impl QueryKey<CompilerContext> for ActiveModuleItemTreeQuery {
    type Value = ActiveModuleItemTree;

    fn name() -> &'static str {
        "active_module_item_tree"
    }

    fn description(&self) -> String {
        format!("active_module_item_tree({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.active_module_item_tree)(db, self.0)
    }
}

impl QueryKey<CompilerContext> for DeclarationModuleItemTreeQuery {
    type Value = ModuleItemTree;

    fn name() -> &'static str {
        "declaration_module_item_tree"
    }

    fn description(&self) -> String {
        format!("declaration_module_item_tree({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        let _raw_item_tree = db.try_get(ModuleItemTreeQuery(self.0))?;
        Ok(db
            .try_get(DeclarationModuleItemTreeInputQuery(self.0))?
            .as_ref()
            .clone())
    }
}

impl QueryKey<CompilerContext> for DeclarationActiveModuleItemTreeQuery {
    type Value = ActiveModuleItemTree;

    fn name() -> &'static str {
        "declaration_active_module_item_tree"
    }

    fn description(&self) -> String {
        format!("declaration_active_module_item_tree({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        let _raw_item_tree = db.try_get(DeclarationModuleItemTreeQuery(self.0))?;
        Ok(db
            .try_get(DeclarationActiveModuleItemTreeInputQuery(self.0))?
            .as_ref()
            .clone())
    }
}

impl QueryKey<CompilerContext> for FullActiveModuleItemTreeQuery {
    type Value = ActiveModuleItemTree;

    fn name() -> &'static str {
        "full_active_module_item_tree"
    }

    fn description(&self) -> String {
        format!("full_active_module_item_tree({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.full_active_module_item_tree)(db, self.0)
    }
}

impl QueryKey<CompilerContext> for ModuleDefsQuery {
    type Value = DefCollection;

    fn name() -> &'static str {
        "module_defs"
    }

    fn description(&self) -> String {
        format!("module_defs({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.module_defs)(db, self.0)
    }
}

impl QueryKey<CompilerContext> for FullModuleDefsQuery {
    type Value = DefCollection;

    fn name() -> &'static str {
        "full_module_defs"
    }

    fn description(&self) -> String {
        format!("full_module_defs({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.full_module_defs)(db, self.0)
    }
}

impl QueryKey<CompilerContext> for PublicSurfaceModuleFactsQuery {
    type Value = PublicSurfaceModuleFacts;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "public_surface_module_facts"
    }

    fn description(&self) -> String {
        format!("public_surface_module_facts({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        db.context()
            .loader_facts()
            .module_public_surface_facts(self.0)
            .ok_or_else(|| db.invalid_input(self, format!("missing loaded module {:?}", self.0)))
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PublicUsingScopesQueryValue {
    pub(super) using_scopes: HashMap<ModuleId, ModuleUsingScope>,
    pub(super) diagnostics: Vec<(ModuleId, Diagnostic)>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PublicSurfacesQueryValue {
    pub(super) surfaces: PublicSurfaces,
    pub(super) diagnostics: Vec<(ModuleId, Diagnostic)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct PublicSurfacesQuery;

impl QueryKey<CompilerContext> for PublicSurfacesQuery {
    type Value = PublicSurfacesValue;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "public_surfaces"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.public_surfaces)(db)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModulePublicSurfaceQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ModulePublicSurfaceQuery {
    type Value = Option<Arc<ModulePublicSurface>>;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "module_public_surface"
    }

    fn description(&self) -> String {
        format!("module_public_surface({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.module_public_surface)(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct PublicSurfaceModuleQuery(pub(super) ModuleId, pub(super) SymbolId);

impl QueryKey<CompilerContext> for PublicSurfaceModuleQuery {
    type Value = Option<StableModuleKey>;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

    fn name() -> &'static str {
        "public_surface_module"
    }

    fn description(&self) -> String {
        format!("public_surface_module({:?}, {:?})", self.0, self.1)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        let surface = db.get(ModulePublicSurfaceQuery(self.0));
        let target = surface.as_ref().as_ref()?.lookup_module(&self.1)?;
        db.get(ModuleGraphQuery).stable_key(target).cloned()
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
        Some(optional_stable_module_key_fingerprint(
            "nia.compiler.public-surface-module.v1",
            value.as_ref(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct PublicSurfaceValueQuery(pub(super) ModuleId, pub(super) SymbolId);

impl QueryKey<CompilerContext> for PublicSurfaceValueQuery {
    type Value = Option<nia_defs::PublicItem>;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "public_surface_value"
    }

    fn description(&self) -> String {
        format!("public_surface_value({:?}, {:?})", self.0, self.1)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.get(ModulePublicSurfaceQuery(self.0))
            .as_ref()
            .as_ref()?
            .lookup_value(&self.1)
            .cloned()
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct PublicSurfaceTypeQuery(pub(super) ModuleId, pub(super) SymbolId);

impl QueryKey<CompilerContext> for PublicSurfaceTypeQuery {
    type Value = Option<nia_defs::PublicItem>;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "public_surface_type"
    }

    fn description(&self) -> String {
        format!("public_surface_type({:?}, {:?})", self.0, self.1)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.get(ModulePublicSurfaceQuery(self.0))
            .as_ref()
            .as_ref()?
            .lookup_type(&self.1)
            .cloned()
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct PublicUsingScopesQuery;

impl QueryKey<CompilerContext> for PublicUsingScopesQuery {
    type Value = PublicUsingScopesValue;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "public_using_scopes"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.public_using_scopes)(db)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleUsingScopeQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ModuleUsingScopeQuery {
    type Value = ModuleUsingScope;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "module_using_scope"
    }

    fn description(&self) -> String {
        format!("module_using_scope({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.module_using_scope)(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct UsingScopeModuleQuery(pub(super) ModuleId, pub(super) SymbolId);

impl QueryKey<CompilerContext> for UsingScopeModuleQuery {
    type Value = Option<StableModuleKey>;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

    fn name() -> &'static str {
        "using_scope_module"
    }

    fn description(&self) -> String {
        format!("using_scope_module({:?}, {:?})", self.0, self.1)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        let scope = db.get(ModuleUsingScopeQuery(self.0));
        let target = scope.lookup_module(&self.1)?;
        db.get(ModuleGraphQuery).stable_key(target).cloned()
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
        Some(optional_stable_module_key_fingerprint(
            "nia.compiler.using-scope-module.v1",
            value.as_ref(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct UsingScopeValueQuery(pub(super) ModuleId, pub(super) SymbolId);

impl QueryKey<CompilerContext> for UsingScopeValueQuery {
    type Value = Option<nia_defs::UsingEntry>;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "using_scope_value"
    }

    fn description(&self) -> String {
        format!("using_scope_value({:?}, {:?})", self.0, self.1)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.get(ModuleUsingScopeQuery(self.0))
            .lookup_value(&self.1)
            .cloned()
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct UsingScopeTypeQuery(pub(super) ModuleId, pub(super) SymbolId);

impl QueryKey<CompilerContext> for UsingScopeTypeQuery {
    type Value = Option<nia_defs::UsingEntry>;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "using_scope_type"
    }

    fn description(&self) -> String {
        format!("using_scope_type({:?}, {:?})", self.0, self.1)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.get(ModuleUsingScopeQuery(self.0))
            .lookup_type(&self.1)
            .cloned()
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct UsingScopeUnresolvedQuery(pub(super) ModuleId, pub(super) SymbolId);

impl QueryKey<CompilerContext> for UsingScopeUnresolvedQuery {
    type Value = bool;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "using_scope_unresolved"
    }

    fn description(&self) -> String {
        format!("using_scope_unresolved({:?}, {:?})", self.0, self.1)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.get(ModuleUsingScopeQuery(self.0))
            .has_unresolved_name(&self.1)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct TypeExposureIndexQuery;

impl QueryKey<CompilerContext> for TypeExposureIndexQuery {
    type Value = TypeExposureIndexValue;

    fn name() -> &'static str {
        "type_exposure_index"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.type_exposure_index)(db)
    }
}
