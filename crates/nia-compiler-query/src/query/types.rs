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
pub(super) struct TypeNormalizationQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct DeclarationTypeNormalizationQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for TypeNormalizationQuery {
    type Value = TypeNormalization;

    fn name() -> &'static str {
        "type_normalization"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.type_normalization)(db, self.0)
    }
}

impl QueryKey<CompilerContext> for DeclarationTypeNormalizationQuery {
    type Value = TypeNormalization;

    fn name() -> &'static str {
        "declaration_type_normalization"
    }

    fn description(&self) -> String {
        format!("declaration_type_normalization({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.declaration_type_normalization)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramTypeNormalizationsQuery;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramDeclarationTypeNormalizationsQuery;

impl QueryKey<CompilerContext> for ProgramTypeNormalizationsQuery {
    type Value = ProgramTypeNormalizations;

    fn name() -> &'static str {
        "program_type_normalizations"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.program_type_normalizations)(db)
    }
}

impl QueryKey<CompilerContext> for ProgramDeclarationTypeNormalizationsQuery {
    type Value = ProgramTypeNormalizations;

    fn name() -> &'static str {
        "program_declaration_type_normalizations"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context()
            .providers
            .program_declaration_type_normalizations)(db)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProgramSignatures {
    pub(super) functions: HashMap<GlobalDefId, ProgramFunctionSignature>,
    pub(super) globals: HashMap<GlobalDefId, ProgramGlobalSignature>,
    pub(super) comptimes: HashMap<GlobalDefId, ProgramComptimeSignature>,
    pub(super) structs: HashMap<GlobalDefId, ProgramStructSignature>,
    pub(super) unions: HashMap<GlobalDefId, ProgramUnionSignature>,
    pub(super) enums: HashMap<GlobalDefId, ProgramEnumSignature>,
    pub(super) traits: HashMap<GlobalDefId, ProgramTraitSignature>,
    pub(super) type_aliases: HashMap<GlobalDefId, nia_item_signatures::ProgramTypeAliasSignature>,
    pub(super) trait_impls: Vec<nia_item_signatures::ProgramTraitImplSignature>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct LoweredFunctionBodies {
    pub(super) interner: nia_ty::TyInterner,
    pub(super) bodies: HashMap<GlobalDefId, nia_function_ir::FunctionBody>,
    pub(super) diagnostics: Vec<nia_function_lower::FunctionLoweringDiagnostic>,
}

impl ProgramSignatures {
    pub(super) fn maps(&self) -> ProgramSignatureMaps<'_> {
        ProgramSignatureMaps {
            functions: &self.functions,
            globals: &self.globals,
            comptimes: &self.comptimes,
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
pub(super) struct ProgramSignaturesQuery;

impl QueryKey<CompilerContext> for ProgramSignaturesQuery {
    type Value = ProgramSignaturesValue;

    fn name() -> &'static str {
        "program_signatures"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.program_signatures)(db)
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
