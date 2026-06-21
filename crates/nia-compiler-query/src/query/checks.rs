// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ComptimeModuleQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ComptimeModuleQuery {
    type Value = ComptimeModuleLowering;

    fn name() -> &'static str {
        "comptime_module"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.comptime_module)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramComptimeModulesQuery;

impl QueryKey<CompilerContext> for ProgramComptimeModulesQuery {
    type Value = ProgramComptimeModules;

    fn name() -> &'static str {
        "program_comptime_modules"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.program_comptime_modules)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramSourcePathsQuery;

impl QueryKey<CompilerContext> for ProgramSourcePathsQuery {
    type Value = ProgramSourcePaths;

    fn name() -> &'static str {
        "program_source_paths"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.program_source_paths)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ComptimeQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ComptimeQuery {
    type Value = ComptimeCheck;

    fn name() -> &'static str {
        "comptime"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.comptime)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramComptimeQuery;

impl QueryKey<CompilerContext> for ProgramComptimeQuery {
    type Value = ProgramComptimeById;

    fn name() -> &'static str {
        "program_comptime"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.program_comptime)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct LayoutsQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for LayoutsQuery {
    type Value = nia_layout::Layouts;

    fn name() -> &'static str {
        "layouts"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.layouts)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct AbiCheckQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for AbiCheckQuery {
    type Value = nia_abi_check::AbiCheck;

    fn name() -> &'static str {
        "abi_check"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.abi_check)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct StaticCheckQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for StaticCheckQuery {
    type Value = nia_static_check::StaticCheck;

    fn name() -> &'static str {
        "static_check"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.static_check)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct FlowCheckQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for FlowCheckQuery {
    type Value = nia_flow_check::FlowCheck;

    fn name() -> &'static str {
        "flow_check"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.flow_check)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct BodyCheckQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for BodyCheckQuery {
    type Value = nia_body_check::BodyCheck;

    fn name() -> &'static str {
        "body_check"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.body_check)(db, self.0)
    }
}
