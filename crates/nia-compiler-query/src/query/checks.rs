// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ComptimeModuleQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for ComptimeModuleQuery {
    type Value = ComptimeModuleLowering;

    fn name() -> &'static str {
        "comptime_module"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.comptime_module)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramComptimeModulesQuery;

impl QueryKey<DriverContext> for ProgramComptimeModulesQuery {
    type Value = HashMap<ModuleId, ResolvedComptimeModule>;

    fn name() -> &'static str {
        "program_comptime_modules"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.program_comptime_modules)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ComptimeQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for ComptimeQuery {
    type Value = ComptimeCheck;

    fn name() -> &'static str {
        "comptime"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.comptime)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramComptimeQuery;

impl QueryKey<DriverContext> for ProgramComptimeQuery {
    type Value = HashMap<ModuleId, ComptimeCheck>;

    fn name() -> &'static str {
        "program_comptime"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.program_comptime)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct LayoutsQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for LayoutsQuery {
    type Value = nia_layout::Layouts;

    fn name() -> &'static str {
        "layouts"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.layouts)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramLayoutsQuery;

impl QueryKey<DriverContext> for ProgramLayoutsQuery {
    type Value = HashMap<ModuleId, nia_layout::Layouts>;

    fn name() -> &'static str {
        "program_layouts"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.program_layouts)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct AbiCheckQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for AbiCheckQuery {
    type Value = nia_abi_check::AbiCheck;

    fn name() -> &'static str {
        "abi_check"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.abi_check)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct StaticCheckQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for StaticCheckQuery {
    type Value = nia_static_check::StaticCheck;

    fn name() -> &'static str {
        "static_check"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.static_check)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct FlowCheckQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for FlowCheckQuery {
    type Value = nia_flow_check::FlowCheck;

    fn name() -> &'static str {
        "flow_check"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.flow_check)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct BodyCheckQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for BodyCheckQuery {
    type Value = nia_body_check::BodyCheck;

    fn name() -> &'static str {
        "body_check"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.body_check)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct BodyIrQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for BodyIrQuery {
    type Value = nia_body_ir::BodyIr;

    fn name() -> &'static str {
        "body_ir"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.body_ir)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SemanticFactsQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for SemanticFactsQuery {
    type Value = nia_sema_ir::SemanticFacts;

    fn name() -> &'static str {
        "semantic_facts"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.semantic_facts)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct BodyDiagnosticsQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for BodyDiagnosticsQuery {
    type Value = Vec<Diagnostic>;

    fn name() -> &'static str {
        "body_diagnostics"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.body_diagnostics)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct FunctionBodiesQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for FunctionBodiesQuery {
    type Value = LoweredFunctionBodies;

    fn name() -> &'static str {
        "function_bodies"
    }

    fn description(&self) -> String {
        format!("function_bodies({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.function_bodies)(db, self.0)
    }
}
