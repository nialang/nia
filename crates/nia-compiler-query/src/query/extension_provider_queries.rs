// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionSignatureModuleInputQueryValue {
    pub(super) module_id: ModuleId,
    pub(super) lowering: TypeLowering,
    pub(super) signatures: ItemSignatures,
    pub(super) defs: DefCollection,
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
pub(super) struct ProgramTraitSolvingSignatures {
    pub(super) enums: HashMap<GlobalDefId, ProgramEnumSignature>,
    pub(super) trait_impls: Vec<nia_item_signatures::ProgramTraitImplSignature>,
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
    pub(super) trait_impls: Vec<nia_item_signatures::ProgramTraitImplSignature>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionProviderModuleFactsQueryValue {
    pub(super) methods: nia_defs::ExtensionMethods,
    pub(super) associated_values: nia_defs::ExtensionAssociatedValues,
    pub(super) associated_value_diagnostics: Vec<Diagnostic>,
    pub(super) trait_impls: Vec<nia_item_signatures::ProgramTraitImplSignature>,
    pub(super) nominal_providers: Vec<crate::program_signatures::NominalExtensionProviderEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionProviderValidationFactsQueryValue {
    pub(super) methods: nia_defs::ExtensionMethods,
    pub(super) diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionProviderValidationProgramFactsQueryValue {
    pub(super) methods: nia_defs::ExtensionMethods,
    pub(super) diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionProviderNominalModuleFactsQueryValue {
    pub(super) nominal_providers: Vec<crate::program_signatures::NominalExtensionProviderEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionProviderNominalCandidateIndexQueryValue(
    pub(super) nia_provider_summary::NominalProviderCandidateIndex<ModuleId>,
);

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionProviderNominalModulesQueryValue {
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
pub(super) struct ExtensionProviderValidationProgramFactsQuery;

impl QueryKey<CompilerContext> for ExtensionProviderValidationProgramFactsQuery {
    type Value = ExtensionProviderValidationProgramFactsValue;

    fn name() -> &'static str {
        "extension_provider_validation_program_facts"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context()
            .providers
            .extension_provider_validation_program_facts)(db)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionProviderNominalCandidateIndexQuery;

impl QueryKey<CompilerContext> for ExtensionProviderNominalCandidateIndexQuery {
    type Value = ExtensionProviderNominalCandidateIndexValue;

    fn name() -> &'static str {
        "extension_provider_nominal_candidate_index"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context()
            .providers
            .extension_provider_nominal_candidate_index)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionProviderNominalModulesQuery(pub(super) GlobalDefId, pub(super) ModuleId);

impl QueryKey<CompilerContext> for ExtensionProviderNominalModulesQuery {
    type Value = ExtensionProviderNominalModulesValue;

    fn name() -> &'static str {
        "extension_provider_nominal_modules"
    }

    fn description(&self) -> String {
        format!(
            "extension_provider_nominal_modules({:?}, {:?})",
            self.0, self.1
        )
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.extension_provider_nominal_modules)(db, self.0, self.1)
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

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionMethodSetQueryValue {
    pub(super) methods: nia_defs::ExtensionMethods,
    pub(super) diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionMethodSetQuery;

impl QueryKey<CompilerContext> for ExtensionMethodSetQuery {
    type Value = ExtensionMethodSetValue;

    fn name() -> &'static str {
        "extension_method_set"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.extension_method_set)(db)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionAssociatedValuesQueryValue {
    pub(super) values: nia_defs::ExtensionAssociatedValues,
    pub(super) diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionAssociatedValuesQuery;

impl QueryKey<CompilerContext> for ExtensionAssociatedValuesQuery {
    type Value = ExtensionAssociatedValuesValue;

    fn name() -> &'static str {
        "extension_associated_values"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.extension_associated_values)(db)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionMethodsQueryValue {
    pub(super) methods: nia_defs::ExtensionMethods,
    pub(super) associated_values: nia_defs::ExtensionAssociatedValues,
    pub(super) diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionMethodsQuery;

impl QueryKey<CompilerContext> for ExtensionMethodsQuery {
    type Value = ExtensionMethodsValue;

    fn name() -> &'static str {
        "extension_methods"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.extension_methods)(db)
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
