// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ConstModuleQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ConstModuleQuery {
    type Value = ConstModuleLowering;

    fn name() -> &'static str {
        "const_module"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.const_module)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ConstQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ConstQuery {
    type Value = ConstCheck;

    fn name() -> &'static str {
        "const"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.const_eval)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ConstArrayLengthsQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ConstArrayLengthsQuery {
    type Value = nia_const_check::ConstArrayLengths;

    fn name() -> &'static str {
        "const_array_lengths"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.const_array_lengths)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ConstEnumValuesQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ConstEnumValuesQuery {
    type Value = nia_const_check::ConstEnumValues;

    fn name() -> &'static str {
        "const_enum_values"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.const_enum_values)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ConstValuesQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ConstValuesQuery {
    type Value = nia_const_check::ConstValues;

    fn name() -> &'static str {
        "const_values"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.const_values)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ConstTypedFactsQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ConstTypedFactsQuery {
    type Value = nia_const_check::ConstTypedFacts;

    fn name() -> &'static str {
        "const_typed_facts"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.const_typed_facts)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct LayoutsQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for LayoutsQuery {
    type Value = nia_layout::Layouts;

    fn name() -> &'static str {
        "layouts"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
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

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
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

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
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

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
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

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
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

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.body_check)(db, self.0)
    }
}
