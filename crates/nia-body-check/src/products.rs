// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, PartialEq)]
pub struct BodyCheck {
    pub ir: Arc<BodyIr>,
    pub facts: Arc<SemanticFacts>,
    pub static_init_refs: HashMap<GlobalDefId, nia_static_ir::StaticInitRefs>,
    pub checked_functions: HashSet<GlobalDefId>,
    pub provider_demands: Arc<HashSet<ProviderDemand>>,
    pub provider_demands_by_function: HashMap<GlobalDefId, HashSet<ProviderDemand>>,
    pub diagnostic_owners: Vec<Option<GlobalDefId>>,
    pub diagnostics: Arc<Vec<Diagnostic>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyCheckProduct {
    Full,
    FactsOnly,
    BodyOnly,
    StaticInitOnly,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PrecheckedBodyCheck {
    pub ir: BodyIr,
    pub facts: SemanticFacts,
    pub checked_functions: HashSet<GlobalDefId>,
    pub diagnostic_owners: Vec<Option<GlobalDefId>>,
    pub diagnostics: Vec<Diagnostic>,
}
