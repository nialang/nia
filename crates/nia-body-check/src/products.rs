// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, PartialEq)]
/// Outputs produced by one body-check query.
pub struct BodyCheck {
    /// Typed Body IR, when the selected product emits it.
    pub ir: Arc<BodyIr>,
    /// Semantic facts collected from checked expressions and calls.
    pub facts: Arc<SemanticFacts>,
    /// Complete executable references from static initializers by global identity.
    pub static_init_refs: HashMap<GlobalDefId, nia_function_ir::FunctionBodyRefs>,
    /// Function identities checked by this product.
    pub checked_functions: HashSet<GlobalDefId>,
    /// Cross-module provider facts demanded during checking.
    pub provider_demands: Arc<HashSet<ProviderDemand>>,
    /// Provider demands grouped by owning function.
    pub provider_demands_by_function: HashMap<GlobalDefId, HashSet<ProviderDemand>>,
    /// Diagnostic ownership aligned with the diagnostics vector.
    pub diagnostic_owners: Vec<Option<GlobalDefId>>,
    /// Diagnostics emitted by body checking.
    pub diagnostics: Arc<Vec<Diagnostic>>,
    /// Checked default expressions keyed by their field definition identity.
    pub field_default_templates: Arc<HashMap<GlobalDefId, nia_body_ir::TypedExpr>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Selects which body-check product is materialized.
pub enum BodyCheckProduct {
    /// Emit facts, Body IR, and static initializer products.
    Full,
    /// Emit semantic facts only.
    FactsOnly,
    /// Emit Body IR without static initializer products.
    BodyOnly,
    /// Emit static initializer products only.
    StaticInitOnly,
}

#[derive(Debug, Clone, PartialEq)]
/// Prior body-check outputs available for incremental reuse.
pub struct PrecheckedBodyCheck {
    /// Previously produced Body IR.
    pub ir: BodyIr,
    /// Previously collected semantic facts.
    pub facts: SemanticFacts,
    /// Previously checked function identities.
    pub checked_functions: HashSet<GlobalDefId>,
    /// Diagnostic ownership for retained diagnostics.
    pub diagnostic_owners: Vec<Option<GlobalDefId>>,
    /// Previously emitted diagnostics.
    pub diagnostics: Vec<Diagnostic>,
    /// Previously produced checked field-default templates.
    pub field_default_templates: HashMap<GlobalDefId, nia_body_ir::TypedExpr>,
}
