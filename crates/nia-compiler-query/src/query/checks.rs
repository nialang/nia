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
    type Value = ModuleConstCheck;

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
    type Value = ModuleLayouts;

    fn name() -> &'static str {
        "layouts"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.layouts)(db, self.0)
    }
}

/// Whether a concrete type that no layout product holds can be laid out in
/// the context of a module, which decides `Sized` for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ConcreteTypeLayoutQuery(pub(super) ModuleId, pub(super) InternedTyId);

impl QueryKey<CompilerContext> for ConcreteTypeLayoutQuery {
    type Value = bool;

    fn name() -> &'static str {
        "concrete_type_layout"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.concrete_type_layout)(db, self.0, self.1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SignatureLayoutsQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for SignatureLayoutsQuery {
    type Value = ModuleLayouts;

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
    type Value = ModuleAbiCheck;

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
    type Value = ModuleStaticCheck;

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
    type Value = ModuleFlowCheck;

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
    type Value = ModuleBodyCheck;

    fn name() -> &'static str {
        "body_check"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.body_check)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct InferredModuleValueTypesQuery(pub(super) ModuleId);

/// Shared runtime type facts for a module's globals and top-level constants.
///
/// Initializer checking remains declaration-kind-specific; only the resolved
/// value types and their dependency fixed point are shared.
#[derive(Debug, Clone, Default)]
pub(super) struct InferredModuleValueTypes {
    pub(super) global_types: HashMap<GlobalDefId, InternedTyId>,
    pub(super) const_types: HashMap<GlobalDefId, InternedTyId>,
    pub(super) global_type_issues:
        HashMap<GlobalDefId, nia_item_signatures::ProgramGlobalTypeIssue>,
}

impl QueryKey<CompilerContext> for InferredModuleValueTypesQuery {
    type Value = InferredModuleValueTypes;

    fn name() -> &'static str {
        "inferred_module_value_types"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        providers::provide_inferred_module_value_types(db, self.0)
    }
}
