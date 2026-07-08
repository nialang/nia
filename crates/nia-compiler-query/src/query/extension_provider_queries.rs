// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_symbol::SymbolId;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionSignatureModuleInputQueryValue {
    pub(super) module_id: ModuleId,
    pub(super) lowering: Arc<TypeLowering>,
    pub(super) signatures: ItemSignatures,
    pub(super) defs: Arc<DefCollection>,
    pub(super) function_signatures: ItemSignatures,
    pub(super) type_signatures: ItemSignatures,
    pub(super) normalization: TypeNormalization,
}

impl ExtensionSignatureModuleInputQueryValue {
    pub(super) fn module(&self) -> ExtensionModuleInput<'_> {
        ExtensionModuleInput {
            module_id: self.module_id,
            defs: &self.defs,
            lowering: &self.lowering,
            signatures: &self.signatures,
            function_signatures: &self.function_signatures,
            type_signatures: &self.type_signatures,
            normalization: &self.normalization,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionTraitSolvingModuleFactsQueryValue {
    pub(super) trait_impls: Vec<nia_item_signatures::ProgramTraitImplSignature>,
    pub(super) invalid_trait_impl_method_ids: HashSet<GlobalDefId>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionTraitImplsForTraitQueryValue {
    pub(super) trait_impls: Vec<nia_item_signatures::ProgramTraitImplSignature>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProgramTraitSolvingSignatures {
    pub(super) trait_impls: Vec<nia_item_signatures::ProgramTraitImplSignature>,
    pub(super) trait_impl_index: nia_item_signatures::ProgramTraitImplIndex,
    pub(super) invalid_trait_impl_method_ids: HashSet<GlobalDefId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionProviderSummaryQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ExtensionProviderSummaryQuery {
    type Value = nia_provider_summary::ProviderSummary;

    fn name() -> &'static str {
        "extension_provider_summary"
    }

    fn description(&self) -> String {
        format!("extension_provider_summary({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.extension_provider_summary)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionSignatureModuleInputQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ExtensionSignatureModuleInputQuery {
    type Value = ExtensionSignatureModuleInputValue;

    fn name() -> &'static str {
        "extension_signature_module_input"
    }

    fn description(&self) -> String {
        format!("extension_signature_module_input({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.extension_signature_module_input)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionTraitSolvingModuleFactsQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ExtensionTraitSolvingModuleFactsQuery {
    type Value = ExtensionTraitSolvingModuleFactsValue;

    fn name() -> &'static str {
        "extension_trait_solving_module_facts"
    }

    fn description(&self) -> String {
        format!("extension_trait_solving_module_facts({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.extension_trait_solving_module_facts)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionTraitImplsForTraitQuery(pub(super) nia_ty::TraitId);

impl QueryKey<CompilerContext> for ExtensionTraitImplsForTraitQuery {
    type Value = ExtensionTraitImplsForTraitValue;

    fn name() -> &'static str {
        "extension_trait_impls_for_trait"
    }

    fn description(&self) -> String {
        format!("extension_trait_impls_for_trait({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.extension_trait_impls_for_trait)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramTraitSolvingSignaturesQuery;

impl QueryKey<CompilerContext> for ProgramTraitSolvingSignaturesQuery {
    type Value = Arc<ProgramTraitSolvingSignatures>;

    fn name() -> &'static str {
        "program_trait_solving_signatures"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.program_trait_solving_signatures)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionProviderDiscoveryIndexQuery;

impl QueryKey<CompilerContext> for ExtensionProviderDiscoveryIndexQuery {
    type Value = ExtensionProviderDiscoveryIndexValue;

    fn name() -> &'static str {
        "extension_provider_discovery_index"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.extension_provider_discovery_index)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionProviderModuleIdsQuery;

impl QueryKey<CompilerContext> for ExtensionProviderModuleIdsQuery {
    type Value = Vec<ModuleId>;

    fn name() -> &'static str {
        "extension_provider_module_ids"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.extension_provider_module_ids)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionProviderModuleEligibilityQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ExtensionProviderModuleEligibilityQuery {
    type Value = bool;

    fn name() -> &'static str {
        "extension_provider_module_eligibility"
    }

    fn description(&self) -> String {
        format!("extension_provider_module_eligibility({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.extension_provider_module_eligibility)(db, self.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionMethodIndexQueryValue {
    pub(super) methods: nia_defs::ExtensionMethods,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionMethodsNamedQueryValue {
    pub(super) methods: Vec<nia_defs::ExtensionMethod>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionMethodByIdQueryValue {
    pub(super) method: Option<nia_defs::ExtensionMethod>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionProviderModuleFactsQueryValue {
    pub(super) methods: nia_defs::ExtensionMethods,
    pub(super) associated_values: nia_defs::ExtensionAssociatedValues,
    pub(super) associated_value_diagnostics: Vec<Diagnostic>,
    pub(super) nominal_providers: Vec<nia_program_signatures::NominalExtensionProviderEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionProviderValidationFactsQueryValue {
    pub(super) methods: nia_defs::ExtensionMethods,
    pub(super) diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionProviderNominalModuleFactsQueryValue {
    pub(super) nominal_providers: Vec<nia_program_signatures::NominalExtensionProviderEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionProviderDiscoveryIndexQueryValue {
    pub(super) provider_modules: Vec<ModuleId>,
    pub(super) nominal_candidates_by_name: HashMap<SymbolId, Vec<ModuleId>>,
    pub(super) method_candidates_by_name: HashMap<SymbolId, Vec<ModuleId>>,
    pub(super) trait_impl_candidates_by_name: HashMap<SymbolId, Vec<ModuleId>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ExtensionProviderNominalTargetNames(pub(super) Vec<SymbolId>);

impl ExtensionProviderNominalTargetNames {
    pub(super) fn new(mut names: Vec<SymbolId>) -> Self {
        names.sort();
        names.dedup();
        Self(names)
    }

    pub(super) fn as_slice(&self) -> &[SymbolId] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionProviderNominalCandidateModulesQueryValue {
    pub(super) modules: Vec<ModuleId>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionProviderNominalModulesForTargetsQueryValue {
    pub(super) modules: Vec<ModuleId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionProviderModuleFactsQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ExtensionProviderModuleFactsQuery {
    type Value = ExtensionProviderModuleFactsValue;

    fn name() -> &'static str {
        "extension_provider_module_facts"
    }

    fn description(&self) -> String {
        format!("extension_provider_module_facts({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.extension_provider_module_facts)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionProviderValidationFactsQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ExtensionProviderValidationFactsQuery {
    type Value = ExtensionProviderValidationFactsValue;

    fn name() -> &'static str {
        "extension_provider_validation_facts"
    }

    fn description(&self) -> String {
        format!("extension_provider_validation_facts({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.extension_provider_validation_facts)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionProviderNominalModuleFactsQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ExtensionProviderNominalModuleFactsQuery {
    type Value = ExtensionProviderNominalModuleFactsValue;

    fn name() -> &'static str {
        "extension_provider_nominal_module_facts"
    }

    fn description(&self) -> String {
        format!("extension_provider_nominal_module_facts({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context()
            .providers
            .extension_provider_nominal_module_facts)(db, self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ExtensionProviderNominalCandidateModulesQuery(
    pub(super) ExtensionProviderNominalTargetNames,
);

impl QueryKey<CompilerContext> for ExtensionProviderNominalCandidateModulesQuery {
    type Value = ExtensionProviderNominalCandidateModulesValue;

    fn name() -> &'static str {
        "extension_provider_nominal_candidate_modules"
    }

    fn description(&self) -> String {
        format!(
            "extension_provider_nominal_candidate_modules({} names)",
            self.0.as_slice().len()
        )
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context()
            .providers
            .extension_provider_nominal_candidate_modules)(db, self.0.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ExtensionProviderNominalTargets(pub(super) Vec<GlobalDefId>);

impl ExtensionProviderNominalTargets {
    pub(super) fn new(mut targets: Vec<GlobalDefId>) -> Self {
        targets.sort();
        targets.dedup();
        Self(targets)
    }

    pub(super) fn as_slice(&self) -> &[GlobalDefId] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ExtensionProviderNominalModulesForTargetsQuery(
    pub(super) ExtensionProviderNominalTargets,
    pub(super) ModuleId,
);

impl QueryKey<CompilerContext> for ExtensionProviderNominalModulesForTargetsQuery {
    type Value = ExtensionProviderNominalModulesForTargetsValue;

    fn name() -> &'static str {
        "extension_provider_nominal_modules_for_targets"
    }

    fn description(&self) -> String {
        format!(
            "extension_provider_nominal_modules_for_targets({} targets, {:?})",
            self.0.as_slice().len(),
            self.1
        )
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context()
            .providers
            .extension_provider_nominal_modules_for_targets)(db, self.0.clone(), self.1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionMethodIndexQuery;

impl QueryKey<CompilerContext> for ExtensionMethodIndexQuery {
    type Value = ExtensionMethodIndexValue;

    fn name() -> &'static str {
        "extension_method_index"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.extension_method_index)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionMethodsNamedQuery(pub(super) SymbolId);

impl QueryKey<CompilerContext> for ExtensionMethodsNamedQuery {
    type Value = ExtensionMethodsNamedValue;

    fn name() -> &'static str {
        "extension_methods_named"
    }

    fn description(&self) -> String {
        format!("extension_methods_named({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.extension_methods_named)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionMethodByIdQuery(pub(super) GlobalDefId);

impl QueryKey<CompilerContext> for ExtensionMethodByIdQuery {
    type Value = ExtensionMethodByIdValue;

    fn name() -> &'static str {
        "extension_method_by_id"
    }

    fn description(&self) -> String {
        format!("extension_method_by_id({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.extension_method_by_id)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionTraitSignatureIndexQuery;

impl QueryKey<CompilerContext> for ExtensionTraitSignatureIndexQuery {
    type Value = ExtensionTraitSignatureIndexValue;

    fn name() -> &'static str {
        "extension_trait_signature_index"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.extension_trait_signature_index)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct VisibleExtensionsQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for VisibleExtensionsQuery {
    type Value = VisibleExtensionsValue;

    fn name() -> &'static str {
        "visible_extensions"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.visible_extensions)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct VisibleTraitImplsQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for VisibleTraitImplsQuery {
    type Value = VisibleTraitImplsValue;

    fn name() -> &'static str {
        "visible_trait_impls"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.visible_trait_impls)(db, self.0)
    }
}
