// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramSignatureModuleIdsQuery(pub(super) nia_item_tree::SignatureItemSet);

impl QueryKey<CompilerContext> for ProgramSignatureModuleIdsQuery {
    type Value = Vec<ModuleId>;

    fn name() -> &'static str {
        "program_signature_module_ids"
    }

    fn description(&self) -> String {
        format!("program_signature_module_ids({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.program_signature_module_ids)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramSignatureModuleEligibilityQuery(
    pub(super) ModuleId,
    pub(super) nia_item_tree::SignatureItemSet,
);

impl QueryKey<CompilerContext> for ProgramSignatureModuleEligibilityQuery {
    type Value = bool;

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
pub(super) struct ProgramVisibleTypeSignatures {
    pub(super) type_aliases: HashMap<GlobalDefId, ProgramTypeAliasSignature>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProgramExecutableSignatures {
    pub(super) functions: HashMap<GlobalDefId, ProgramFunctionSignature>,
    pub(super) globals: HashMap<GlobalDefId, ProgramGlobalSignature>,
    pub(super) comptimes: HashMap<GlobalDefId, ProgramComptimeSignature>,
    pub(super) structs: HashMap<GlobalDefId, ProgramStructSignature>,
    pub(super) unions: HashMap<GlobalDefId, ProgramUnionSignature>,
    pub(super) enums: HashMap<GlobalDefId, ProgramEnumSignature>,
    pub(super) type_aliases: HashMap<GlobalDefId, nia_item_signatures::ProgramTypeAliasSignature>,
    pub(super) traits: HashMap<GlobalDefId, ProgramTraitSignature>,
    pub(super) trait_impls: Vec<nia_item_signatures::ProgramTraitImplSignature>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProgramBackendSignatures {
    pub(super) functions: HashMap<GlobalDefId, ProgramFunctionSignature>,
    pub(super) structs: HashMap<GlobalDefId, ProgramStructSignature>,
    pub(super) unions: HashMap<GlobalDefId, ProgramUnionSignature>,
    pub(super) enums: HashMap<GlobalDefId, ProgramEnumSignature>,
    pub(super) traits: HashMap<GlobalDefId, ProgramTraitSignature>,
    pub(super) type_aliases: HashMap<GlobalDefId, ProgramTypeAliasSignature>,
    pub(super) trait_impls: Vec<nia_item_signatures::ProgramTraitImplSignature>,
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
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProgramAbiSignaturesValue {
    pub(super) structs: HashMap<GlobalDefId, StructSignature>,
    pub(super) unions: HashMap<GlobalDefId, UnionSignature>,
    pub(super) enums: HashMap<GlobalDefId, nia_item_signatures::EnumSignature>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct LoweredFunctionBodies {
    pub(super) interner: nia_ty::TyInterner,
    pub(super) bodies: HashMap<GlobalDefId, nia_function_ir::FunctionBody>,
    pub(super) diagnostics: Vec<nia_function_lower::FunctionLoweringDiagnostic>,
}

impl ProgramExecutableSignatures {
    pub(super) fn codegen_maps(&self) -> ProgramCodegenSignatures<'_> {
        ProgramCodegenSignatures {
            functions: &self.functions,
            structs: &self.structs,
            unions: &self.unions,
            enums: &self.enums,
            traits: &self.traits,
            type_aliases: &self.type_aliases,
            trait_impls: &self.trait_impls,
        }
    }
}

impl ProgramBackendSignatures {
    pub(super) fn codegen_maps(&self) -> ProgramCodegenSignatures<'_> {
        ProgramCodegenSignatures {
            functions: &self.functions,
            structs: &self.structs,
            unions: &self.unions,
            enums: &self.enums,
            traits: &self.traits,
            type_aliases: &self.type_aliases,
            trait_impls: &self.trait_impls,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramVisibleTypeSignaturesQuery;

impl QueryKey<CompilerContext> for ProgramVisibleTypeSignaturesQuery {
    type Value = Arc<ProgramVisibleTypeSignatures>;

    fn name() -> &'static str {
        "program_visible_type_signatures"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.program_visible_type_signatures)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramBackendSignaturesQuery;

impl QueryKey<CompilerContext> for ProgramBackendSignaturesQuery {
    type Value = Arc<ProgramBackendSignatures>;

    fn name() -> &'static str {
        "program_backend_signatures"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.program_backend_signatures)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramAbiSignaturesQuery;

impl QueryKey<CompilerContext> for ProgramAbiSignaturesQuery {
    type Value = Arc<ProgramAbiSignaturesValue>;

    fn name() -> &'static str {
        "program_abi_signatures"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.program_abi_signatures)(db)
    }
}
