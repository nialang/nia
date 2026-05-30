// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct TypeResolutionQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for TypeResolutionQuery {
    type Value = TypeResolution;

    fn name() -> &'static str {
        "type_resolution"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let loaded = db.query(LoadedModuleQuery(self.0));
        let defs = db.query(ModuleDefsQuery(self.0));
        let all_defs = db.query(DefsByModuleQuery);
        let imports = db.query(ImportAliasMapQuery);
        let public = db.query(PublicSurfaceQuery);
        let empty_using = ModuleUsingScope::default();
        let using_scope = public.using_scopes.get(&self.0).unwrap_or(&empty_using);
        nia_type_resolve::resolve_module_types_with_context(
            &loaded.module,
            &defs,
            &imports,
            &all_defs,
            &public.surfaces,
            using_scope,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct TypeLoweringQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for TypeLoweringQuery {
    type Value = TypeLowering;

    fn name() -> &'static str {
        "type_lowering"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let loaded = db.query(LoadedModuleQuery(self.0));
        let type_resolution = db.query(TypeResolutionQuery(self.0));
        let all_defs = db.query(DefsByModuleQuery);
        nia_type_lower::lower_module_types_with_defs(
            self.0,
            &loaded.module,
            &type_resolution,
            &all_defs,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ItemSignaturesQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for ItemSignaturesQuery {
    type Value = ItemSignatures;

    fn name() -> &'static str {
        "item_signatures"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let loaded = db.query(LoadedModuleQuery(self.0));
        let defs = db.query(ModuleDefsQuery(self.0));
        let type_lowering = db.query(TypeLoweringQuery(self.0));
        nia_item_signatures::collect_item_signatures(&loaded.module, &defs, &type_lowering)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ItemSignaturesByModuleQuery;

impl QueryKey<DriverContext> for ItemSignaturesByModuleQuery {
    type Value = Vec<ItemSignatures>;

    fn name() -> &'static str {
        "item_signatures_by_module"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        db.query_many(
            db.query(ParseOkModuleIdsQuery)
                .into_iter()
                .map(ItemSignaturesQuery),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct TypeLoweringsByModuleQuery;

impl QueryKey<DriverContext> for TypeLoweringsByModuleQuery {
    type Value = Vec<TypeLowering>;

    fn name() -> &'static str {
        "type_lowerings_by_module"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        db.query_many(
            db.query(ParseOkModuleIdsQuery)
                .into_iter()
                .map(TypeLoweringQuery),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct TypeNormalizationQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for TypeNormalizationQuery {
    type Value = TypeNormalization;

    fn name() -> &'static str {
        "type_normalization"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let type_lowering = db.query(TypeLoweringQuery(self.0));
        let item_signatures = db.query(ItemSignaturesQuery(self.0));
        nia_type_normalize::normalize_module_types(
            self.0,
            &type_lowering.interner,
            &item_signatures,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct TypeNormalizationsByModuleQuery;

impl QueryKey<DriverContext> for TypeNormalizationsByModuleQuery {
    type Value = Vec<TypeNormalization>;

    fn name() -> &'static str {
        "type_normalizations_by_module"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        db.query_many(
            db.query(ParseOkModuleIdsQuery)
                .into_iter()
                .map(TypeNormalizationQuery),
        )
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramSignaturesQuery;

impl QueryKey<DriverContext> for ProgramSignaturesQuery {
    type Value = ProgramSignatures;

    fn name() -> &'static str {
        "program_signatures"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let modules = modules_in_order(db);
        let type_lowerings = db.query(TypeLoweringsByModuleQuery);
        let item_signatures = db.query(ItemSignaturesByModuleQuery);
        ProgramSignatures {
            functions: collect_program_functions(&modules, &type_lowerings, &item_signatures),
            globals: collect_program_globals(&modules, &type_lowerings, &item_signatures),
            comptimes: collect_program_comptimes(&modules, &type_lowerings, &item_signatures),
            structs: collect_program_structs(&modules, &type_lowerings, &item_signatures),
            unions: collect_program_unions(&modules, &type_lowerings, &item_signatures),
            enums: collect_program_enums(&modules, &type_lowerings, &item_signatures),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtensionMethodsQueryValue {
    pub(super) methods: nia_defs::ExtensionMethods,
    pub(super) diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExtensionMethodsQuery;

impl QueryKey<DriverContext> for ExtensionMethodsQuery {
    type Value = ExtensionMethodsQueryValue;

    fn name() -> &'static str {
        "extension_methods"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let modules = modules_in_order(db);
        let defs = db.query(DefsByModuleQuery);
        let type_lowerings = db.query(TypeLoweringsByModuleQuery);
        let normalizations = db.query(TypeNormalizationsByModuleQuery);
        let (methods, diagnostics) =
            collect_extension_methods(&modules, &defs, &type_lowerings, &normalizations);
        ExtensionMethodsQueryValue {
            methods,
            diagnostics,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct VisibleExtensionsQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for VisibleExtensionsQuery {
    type Value = VisibleExtensionsForModule;

    fn name() -> &'static str {
        "visible_extensions"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let imports = db.query(ImportAliasMapQuery);
        let defs = db.query(DefsByModuleQuery);
        let normalizations = db.query(TypeNormalizationsByModuleQuery);
        let extensions = db.query(ExtensionMethodsQuery);
        visible_extensions_for_module(
            self.0,
            &imports,
            &defs,
            &normalizations,
            &extensions.methods,
        )
    }
}
