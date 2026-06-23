// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_node_id::NodeOriginTable;
use nia_parser::ParseError;
use std::collections::HashMap;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CheckedProgramQuery;

impl QueryKey<CompilerContext> for CheckedProgramQuery {
    type Value = CheckedProgram;

    fn name() -> &'static str {
        "checked_program"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.checked_program)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleGraphQuery;

impl QueryKey<CompilerContext> for ModuleGraphQuery {
    type Value = ModuleGraph;

    fn name() -> &'static str {
        "module_graph"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.module_graph)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct LoadedModulesQuery;

impl QueryKey<CompilerContext> for LoadedModulesQuery {
    type Value = Vec<ModuleId>;

    fn name() -> &'static str {
        "loaded_modules"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.context().loaded_modules()
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
        db.context().load_diagnostics()
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
        db.context().target()
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
        db.context().runtime()
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
pub(super) struct CompilerTimingsQuery;

impl QueryKey<CompilerContext> for CompilerTimingsQuery {
    type Value = TimingMode;

    fn name() -> &'static str {
        "compiler_timings"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.context().timings()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ParseOkModuleIdsQuery;

impl QueryKey<CompilerContext> for ParseOkModuleIdsQuery {
    type Value = Vec<ModuleId>;

    fn name() -> &'static str {
        "parse_ok_module_ids"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.parse_ok_module_ids)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModulePathQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ModulePathQuery {
    type Value = SourcePath;

    fn name() -> &'static str {
        "module_path"
    }

    fn description(&self) -> String {
        format!("module_path({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.context().module_path(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleSourceVersionQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ModuleSourceVersionQuery {
    type Value = SourceVersion;

    fn name() -> &'static str {
        "module_source_version"
    }

    fn description(&self) -> String {
        format!("module_source_version({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.context().module_source_version(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleOriginsQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ModuleOriginsQuery {
    type Value = NodeOriginTable;

    fn name() -> &'static str {
        "module_origins"
    }

    fn description(&self) -> String {
        format!("module_origins({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.context().module_origins(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleParseErrorsQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ModuleParseErrorsQuery {
    type Value = Vec<ParseError>;

    fn name() -> &'static str {
        "module_parse_errors"
    }

    fn description(&self) -> String {
        format!("module_parse_errors({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.context().module_parse_errors(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleItemTreeInputQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ModuleItemTreeInputQuery {
    type Value = ModuleItemTree;

    fn name() -> &'static str {
        "module_item_tree_input"
    }

    fn description(&self) -> String {
        format!("module_item_tree_input({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.context().module_item_tree(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ActiveModuleItemTreeInputQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ActiveModuleItemTreeInputQuery {
    type Value = ActiveModuleItemTree;

    fn name() -> &'static str {
        "active_module_item_tree_input"
    }

    fn description(&self) -> String {
        format!("active_module_item_tree_input({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.context().active_module_item_tree(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct FullModuleItemTreeInputQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for FullModuleItemTreeInputQuery {
    type Value = ModuleItemTree;

    fn name() -> &'static str {
        "full_module_item_tree_input"
    }

    fn description(&self) -> String {
        format!("full_module_item_tree_input({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.context().module_item_tree(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct FullActiveModuleItemTreeInputQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for FullActiveModuleItemTreeInputQuery {
    type Value = ActiveModuleItemTree;

    fn name() -> &'static str {
        "full_active_module_item_tree_input"
    }

    fn description(&self) -> String {
        format!("full_active_module_item_tree_input({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.context().active_module_item_tree(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleDefsQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct FullModuleDefsQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleItemTreeQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ActiveModuleItemTreeQuery(pub(super) ModuleId);

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

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
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

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
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

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.active_module_item_tree)(db, self.0)
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

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct DefsByModuleQuery;

impl QueryKey<CompilerContext> for DefsByModuleQuery {
    type Value = Vec<DefCollection>;

    fn name() -> &'static str {
        "defs_by_module"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.defs_by_module)(db)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PublicSurfaceQueryValue {
    pub(super) surfaces: PublicSurfaces,
    pub(super) using_scopes: HashMap<ModuleId, ModuleUsingScope>,
    pub(super) diagnostics: Vec<(ModuleId, Diagnostic)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct PublicSurfaceQuery;

impl QueryKey<CompilerContext> for PublicSurfaceQuery {
    type Value = PublicSurfaceQueryValue;

    fn name() -> &'static str {
        "public_surface"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.public_surface)(db)
    }
}
