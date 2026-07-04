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
pub(super) struct ComptimeArrayLengthsQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ComptimeArrayLengthsQuery {
    type Value = nia_comptime_check::ComptimeArrayLengths;

    fn name() -> &'static str {
        "comptime_array_lengths"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.comptime_array_lengths)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ComptimeEnumValuesQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ComptimeEnumValuesQuery {
    type Value = nia_comptime_check::ComptimeEnumValues;

    fn name() -> &'static str {
        "comptime_enum_values"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.comptime_enum_values)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ComptimeValuesQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ComptimeValuesQuery {
    type Value = nia_comptime_check::ComptimeValues;

    fn name() -> &'static str {
        "comptime_values"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.comptime_values)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ComptimeTypedFactsQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ComptimeTypedFactsQuery {
    type Value = nia_comptime_check::ComptimeTypedFacts;

    fn name() -> &'static str {
        "comptime_typed_facts"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.comptime_typed_facts)(db, self.0)
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
pub(super) struct SignatureLayoutsQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for SignatureLayoutsQuery {
    type Value = nia_layout::Layouts;

    fn name() -> &'static str {
        "signature_layouts"
    }

    fn description(&self) -> String {
        format!("signature_layouts({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.signature_layouts)(db, self.0)
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
