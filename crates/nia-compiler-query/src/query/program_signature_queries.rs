// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramSignatureModuleIdsQuery(pub(super) nia_item_tree::SignatureItemSet);

impl QueryKey<CompilerContext> for ProgramSignatureModuleIdsQuery {
    type Value = StableModuleSequence;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

    fn name() -> &'static str {
        "program_signature_module_ids"
    }

    fn description(&self) -> String {
        format!("program_signature_module_ids({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.program_signature_module_ids)(db, self.0)
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
        Some(stable_module_sequence_fingerprint(
            "nia.compiler.program-signature-module-ids.v1",
            value,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramSignatureModuleEligibilityQuery(
    pub(super) ModuleId,
    pub(super) nia_item_tree::SignatureItemSet,
);

impl QueryKey<CompilerContext> for ProgramSignatureModuleEligibilityQuery {
    type Value = bool;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

    fn name() -> &'static str {
        "program_signature_module_eligibility"
    }

    fn description(&self) -> String {
        format!(
            "program_signature_module_eligibility({:?}, {:?})",
            self.0, self.1
        )
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.program_signature_module_eligibility)(db, self.0, self.1)
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
        Some(bool_query_fingerprint(
            "nia.compiler.program-signature-module-eligibility.v1",
            *value,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleProgramSignatureFactsQuery(
    pub(super) ModuleId,
    pub(super) nia_item_tree::SignatureItemSet,
);

impl QueryKey<CompilerContext> for ModuleProgramSignatureFactsQuery {
    type Value = ModuleProgramSignatureFactsValue;

    fn name() -> &'static str {
        "module_program_signature_facts"
    }

    fn description(&self) -> String {
        format!("module_program_signature_facts({:?}, {:?})", self.0, self.1)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.module_program_signature_facts)(db, self.0, self.1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleAbiSignatureFactsQuery(pub(super) ModuleId);

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ModuleAbiSignatureFactsQueryValue {
    pub(super) structs: HashMap<GlobalDefId, StructSignature>,
    pub(super) unions: HashMap<GlobalDefId, UnionSignature>,
    pub(super) enums: HashMap<GlobalDefId, nia_item_signatures::EnumSignature>,
}

impl QueryKey<CompilerContext> for ModuleAbiSignatureFactsQuery {
    type Value = ModuleAbiSignatureFactsValue;

    fn name() -> &'static str {
        "module_abi_signature_facts"
    }

    fn description(&self) -> String {
        format!("module_abi_signature_facts({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.module_abi_signature_facts)(db, self.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProgramExecutableNonFunctionSignatures {
    pub(super) globals: HashMap<GlobalDefId, ProgramGlobalSignature>,
    pub(super) consts: HashMap<GlobalDefId, ProgramConstSignature>,
    pub(super) structs: HashMap<GlobalDefId, ProgramStructSignature>,
    pub(super) unions: HashMap<GlobalDefId, ProgramUnionSignature>,
    pub(super) enums: HashMap<GlobalDefId, ProgramEnumSignature>,
    pub(super) type_aliases: HashMap<GlobalDefId, nia_item_signatures::ProgramTypeAliasSignature>,
    pub(super) traits: HashMap<GlobalDefId, ProgramTraitSignature>,
    pub(super) trait_impls: Vec<nia_item_signatures::ProgramTraitImplSignature>,
    pub(super) trait_impl_index: nia_item_signatures::ProgramTraitImplIndex,
    pub(super) trait_method_index: nia_program_signatures::ProgramTraitMethodIndex,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ProgramCodegenSignatures<'a> {
    pub(super) functions: &'a HashMap<GlobalDefId, ProgramFunctionSignature>,
    pub(super) structs: &'a HashMap<GlobalDefId, ProgramStructSignature>,
    pub(super) unions: &'a HashMap<GlobalDefId, ProgramUnionSignature>,
    pub(super) enums: &'a HashMap<GlobalDefId, ProgramEnumSignature>,
    pub(super) traits: &'a HashMap<GlobalDefId, ProgramTraitSignature>,
    pub(super) type_aliases: &'a HashMap<GlobalDefId, ProgramTypeAliasSignature>,
    pub(super) trait_impls: &'a [nia_item_signatures::ProgramTraitImplSignature],
    pub(super) trait_impl_index: &'a nia_item_signatures::ProgramTraitImplIndex,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProgramAbiSignaturesValue {
    pub(super) structs: HashMap<GlobalDefId, StructSignature>,
    pub(super) unions: HashMap<GlobalDefId, UnionSignature>,
    pub(super) enums: HashMap<GlobalDefId, nia_item_signatures::EnumSignature>,
}

impl ProgramExecutableNonFunctionSignatures {
    pub(super) fn codegen_maps_with_functions<'a>(
        &'a self,
        functions: &'a HashMap<GlobalDefId, ProgramFunctionSignature>,
    ) -> ProgramCodegenSignatures<'a> {
        ProgramCodegenSignatures {
            functions,
            structs: &self.structs,
            unions: &self.unions,
            enums: &self.enums,
            traits: &self.traits,
            type_aliases: &self.type_aliases,
            trait_impls: &self.trait_impls,
            trait_impl_index: &self.trait_impl_index,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramTraitMethodIndexQuery;

impl QueryKey<CompilerContext> for ProgramTraitMethodIndexQuery {
    type Value = nia_program_signatures::ProgramTraitMethodIndex;

    fn name() -> &'static str {
        "program_trait_method_index"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.program_trait_method_index)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramTypeAliasSignatureQuery(pub(super) GlobalDefId);

impl QueryKey<CompilerContext> for ProgramTypeAliasSignatureQuery {
    type Value = Option<ProgramTypeAliasSignature>;

    fn name() -> &'static str {
        "program_type_alias_signature"
    }

    fn description(&self) -> String {
        format!("program_type_alias_signature({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.get(ModuleProgramSignatureFactsQuery(
            self.0.module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))
        .type_aliases
        .get(&self.0)
        .cloned()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramAbiSignaturesQuery;

impl QueryKey<CompilerContext> for ProgramAbiSignaturesQuery {
    type Value = ProgramAbiSignaturesValue;

    fn name() -> &'static str {
        "program_abi_signatures"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.program_abi_signatures)(db)
    }
}
