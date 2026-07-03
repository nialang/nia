// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct TypeResolutionQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct DeclarationTypeResolutionQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for TypeResolutionQuery {
    type Value = TypeResolution;

    fn name() -> &'static str {
        "type_resolution"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.type_resolution)(db, self.0)
    }
}

impl QueryKey<CompilerContext> for DeclarationTypeResolutionQuery {
    type Value = TypeResolution;

    fn name() -> &'static str {
        "declaration_type_resolution"
    }

    fn description(&self) -> String {
        format!("declaration_type_resolution({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.declaration_type_resolution)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct TypeLoweringQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct DeclarationTypeLoweringQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for TypeLoweringQuery {
    type Value = TypeLowering;

    fn name() -> &'static str {
        "type_lowering"
    }

    fn description(&self) -> String {
        format!("type_lowering({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.type_lowering)(db, self.0)
    }
}

impl QueryKey<CompilerContext> for DeclarationTypeLoweringQuery {
    type Value = TypeLowering;

    fn name() -> &'static str {
        "declaration_type_lowering"
    }

    fn description(&self) -> String {
        format!("declaration_type_lowering({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.declaration_type_lowering)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ItemSignaturesQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ItemSignaturesQuery {
    type Value = ItemSignatures;

    fn name() -> &'static str {
        "item_signatures"
    }

    fn description(&self) -> String {
        format!("item_signatures({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.item_signatures)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SignatureItemTreeQuery(
    pub(super) ModuleId,
    pub(super) nia_item_tree::SignatureItemSet,
);

impl QueryKey<CompilerContext> for SignatureItemTreeQuery {
    type Value = ActiveModuleItemTree;

    fn name() -> &'static str {
        "signature_item_tree"
    }

    fn description(&self) -> String {
        format!("signature_item_tree({:?}, {:?})", self.0, self.1)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.context().signature_item_tree(db, self.0, self.1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SignatureTypeResolutionQuery(
    pub(super) ModuleId,
    pub(super) nia_item_tree::SignatureItemSet,
);

impl QueryKey<CompilerContext> for SignatureTypeResolutionQuery {
    type Value = TypeResolution;

    fn name() -> &'static str {
        "signature_type_resolution"
    }

    fn description(&self) -> String {
        format!("signature_type_resolution({:?}, {:?})", self.0, self.1)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.signature_type_resolution)(db, self.0, self.1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SignatureTypeLoweringQuery(
    pub(super) ModuleId,
    pub(super) nia_item_tree::SignatureItemSet,
);

impl QueryKey<CompilerContext> for SignatureTypeLoweringQuery {
    type Value = TypeLowering;

    fn name() -> &'static str {
        "signature_type_lowering"
    }

    fn description(&self) -> String {
        format!("signature_type_lowering({:?}, {:?})", self.0, self.1)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.signature_type_lowering)(db, self.0, self.1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SignatureItemSignaturesQuery(
    pub(super) ModuleId,
    pub(super) nia_item_tree::SignatureItemSet,
);

impl QueryKey<CompilerContext> for SignatureItemSignaturesQuery {
    type Value = ItemSignatures;

    fn name() -> &'static str {
        "signature_item_signatures"
    }

    fn description(&self) -> String {
        format!("signature_item_signatures({:?}, {:?})", self.0, self.1)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.signature_item_signatures)(db, self.0, self.1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SignatureTypeNormalizationQuery(
    pub(super) ModuleId,
    pub(super) nia_item_tree::SignatureItemSet,
);

impl QueryKey<CompilerContext> for SignatureTypeNormalizationQuery {
    type Value = TypeNormalization;

    fn name() -> &'static str {
        "signature_type_normalization"
    }

    fn description(&self) -> String {
        format!("signature_type_normalization({:?}, {:?})", self.0, self.1)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.signature_type_normalization)(db, self.0, self.1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct TypeNormalizationQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct LayoutTypeNormalizationQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for TypeNormalizationQuery {
    type Value = TypeNormalization;

    fn name() -> &'static str {
        "type_normalization"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.type_normalization)(db, self.0)
    }
}

impl QueryKey<CompilerContext> for LayoutTypeNormalizationQuery {
    type Value = TypeNormalization;

    fn name() -> &'static str {
        "layout_type_normalization"
    }

    fn description(&self) -> String {
        format!("layout_type_normalization({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.layout_type_normalization)(db, self.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProgramBodyFunctionSignatures {
    pub(super) functions: HashMap<GlobalDefId, ProgramFunctionSignature>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProgramBodyValueSignatures {
    pub(super) globals: HashMap<GlobalDefId, ProgramGlobalSignature>,
    pub(super) comptimes: HashMap<GlobalDefId, ProgramComptimeSignature>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProgramBodyTypeSignatures {
    pub(super) structs: HashMap<GlobalDefId, ProgramStructSignature>,
    pub(super) unions: HashMap<GlobalDefId, ProgramUnionSignature>,
    pub(super) enums: HashMap<GlobalDefId, ProgramEnumSignature>,
    pub(super) type_aliases: HashMap<GlobalDefId, nia_item_signatures::ProgramTypeAliasSignature>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProgramBodyTraitSignatures {
    pub(super) traits: HashMap<GlobalDefId, ProgramTraitSignature>,
    pub(super) trait_impls: Vec<nia_item_signatures::ProgramTraitImplSignature>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProgramTraitSolvingSignatures {
    pub(super) enums: HashMap<GlobalDefId, ProgramEnumSignature>,
    pub(super) trait_impls: Vec<nia_item_signatures::ProgramTraitImplSignature>,
    pub(super) invalid_trait_impl_method_ids: HashSet<GlobalDefId>,
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

impl ProgramBodyValueSignatures {
    pub(super) fn body_maps(&self) -> nia_body_check::BodyProgramValueSignatures<'_> {
        nia_body_check::BodyProgramValueSignatures {
            globals: &self.globals,
            comptimes: &self.comptimes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramBodyFunctionSignaturesQuery;

impl ProgramBodyTypeSignatures {
    pub(super) fn body_maps(&self) -> nia_body_check::BodyProgramTypeSignatures<'_> {
        nia_body_check::BodyProgramTypeSignatures {
            structs: &self.structs,
            unions: &self.unions,
            enums: &self.enums,
            type_aliases: &self.type_aliases,
        }
    }
}

impl ProgramBodyTraitSignatures {
    pub(super) fn body_maps(&self) -> nia_body_check::BodyProgramTraitSignatures<'_> {
        nia_body_check::BodyProgramTraitSignatures {
            traits: &self.traits,
            trait_impls: &self.trait_impls,
        }
    }
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

impl QueryKey<CompilerContext> for ProgramBodyFunctionSignaturesQuery {
    type Value = Arc<ProgramBodyFunctionSignatures>;

    fn name() -> &'static str {
        "program_body_function_signatures"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.program_body_function_signatures)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramBodyValueSignaturesQuery;

impl QueryKey<CompilerContext> for ProgramBodyValueSignaturesQuery {
    type Value = Arc<ProgramBodyValueSignatures>;

    fn name() -> &'static str {
        "program_body_value_signatures"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.program_body_value_signatures)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramBodyTypeSignaturesQuery;

impl QueryKey<CompilerContext> for ProgramBodyTypeSignaturesQuery {
    type Value = Arc<ProgramBodyTypeSignatures>;

    fn name() -> &'static str {
        "program_body_type_signatures"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.program_body_type_signatures)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramBodyTraitSignaturesQuery;

impl QueryKey<CompilerContext> for ProgramBodyTraitSignaturesQuery {
    type Value = Arc<ProgramBodyTraitSignatures>;

    fn name() -> &'static str {
        "program_body_trait_signatures"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.program_body_trait_signatures)(db)
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
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionProviderNominalIndexQueryValue {
    pub(super) providers_by_nominal:
        HashMap<GlobalDefId, Vec<crate::program_signatures::NominalExtensionProviderEntry>>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionProviderNominalModuleQueryValue {
    pub(super) providers: Vec<crate::program_signatures::NominalExtensionProviderEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionProviderNominalModuleQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ExtensionProviderNominalModuleQuery {
    type Value = ExtensionProviderNominalModuleValue;

    fn name() -> &'static str {
        "extension_provider_nominal_module"
    }

    fn description(&self) -> String {
        format!("extension_provider_nominal_module({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.extension_provider_nominal_module)(db, self.0)
    }
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
pub(super) struct ExtensionProviderNominalIndexQuery;

impl QueryKey<CompilerContext> for ExtensionProviderNominalIndexQuery {
    type Value = Arc<ExtensionProviderNominalIndexQueryValue>;

    fn name() -> &'static str {
        "extension_provider_nominal_index"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.extension_provider_nominal_index)(db)
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
