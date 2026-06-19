// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
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
    type Value = Vec<LoadedModule>;

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
pub(super) struct LoadedModuleQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for LoadedModuleQuery {
    type Value = LoadedModule;

    fn name() -> &'static str {
        "loaded_module"
    }

    fn description(&self) -> String {
        format!("loaded_module({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.loaded_module)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleDefsQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleItemTreeQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ActiveModuleItemTreeQuery(pub(super) ModuleId);

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramDefsByIdQuery;

impl QueryKey<CompilerContext> for ProgramDefsByIdQuery {
    type Value = ProgramDefsById;

    fn name() -> &'static str {
        "program_defs_by_id"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.program_defs_by_id)(db)
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
