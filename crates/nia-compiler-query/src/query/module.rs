// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct TypeResolutionQuery(ModuleId);

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
pub(super) struct TypeLoweringQuery(ModuleId);

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
pub(super) struct ItemSignaturesQuery(ModuleId);

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
pub(super) struct TypeNormalizationQuery(ModuleId);

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
    fn maps(&self) -> ProgramSignatureMaps<'_> {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ValueResolutionQuery(ModuleId);

impl QueryKey<DriverContext> for ValueResolutionQuery {
    type Value = ValueResolution;

    fn name() -> &'static str {
        "value_resolution"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let loaded = db.query(LoadedModuleQuery(self.0));
        let defs = db.query(ModuleDefsQuery(self.0));
        let all_defs = db.query(DefsByModuleQuery);
        let imports = db.query(ImportAliasMapQuery);
        let public = db.query(PublicSurfaceQuery);
        let empty_using = ModuleUsingScope::default();
        let using_scope = public.using_scopes.get(&self.0).unwrap_or(&empty_using);
        nia_value_resolve::resolve_module_values_with_context(
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
pub(super) struct LocalResolutionQuery(ModuleId);

impl QueryKey<DriverContext> for LocalResolutionQuery {
    type Value = LocalResolution;

    fn name() -> &'static str {
        "local_resolution"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let loaded = db.query(LoadedModuleQuery(self.0));
        let defs = db.query(ModuleDefsQuery(self.0));
        let values = db.query(ValueResolutionQuery(self.0));
        nia_local_resolve::resolve_module_locals(&loaded.module, &defs, &values)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ComptimeQuery(ModuleId);

impl QueryKey<DriverContext> for ComptimeQuery {
    type Value = ComptimeCheck;

    fn name() -> &'static str {
        "comptime"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let loaded = db.query(LoadedModuleQuery(self.0));
        let all_modules = db.query(AllModulesQuery);
        let defs = db.query(ModuleDefsQuery(self.0));
        let all_defs = db.query(DefsByModuleQuery);
        let values = db.query(ValueResolutionQuery(self.0));
        let locals = db.query(LocalResolutionQuery(self.0));
        let item_signatures = db.query(ItemSignaturesQuery(self.0));
        let type_normalization = db.query(TypeNormalizationQuery(self.0));
        let type_lowering = db.query(TypeLoweringQuery(self.0));
        nia_comptime_check::check_module_comptime(nia_comptime_check::ComptimeInput {
            module: &loaded.module,
            all_modules: &all_modules,
            defs: &defs,
            all_defs: &all_defs,
            values: &values,
            locals: &locals,
            signatures: &item_signatures,
            interner: &type_normalization.interner,
            const_exprs: &type_lowering.const_exprs,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramComptimeQuery;

impl QueryKey<DriverContext> for ProgramComptimeQuery {
    type Value = HashMap<ModuleId, ComptimeCheck>;

    fn name() -> &'static str {
        "program_comptime"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let ids = db.query(ParseOkModuleIdsQuery);
        let comptimes = db.query_many(ids.iter().copied().map(ComptimeQuery));
        ids.into_iter().zip(comptimes).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct LayoutsQuery(ModuleId);

impl QueryKey<DriverContext> for LayoutsQuery {
    type Value = nia_layout::Layouts;

    fn name() -> &'static str {
        "layouts"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let defs = db.query(ModuleDefsQuery(self.0));
        let type_normalization = db.query(TypeNormalizationQuery(self.0));
        let item_signatures = db.query(ItemSignaturesQuery(self.0));
        let comptime = db.query(ComptimeQuery(self.0));
        let layout_query = |module_id| Some(db.query(LayoutsQuery(module_id)));
        let comptime_query = |module_id| Some(db.query(ComptimeQuery(module_id)));
        nia_layout::compute_layouts_with_program_context(
            &defs,
            &type_normalization.interner,
            &item_signatures,
            &type_normalization.normalized,
            &comptime,
            nia_layout::TargetDataLayout::LP64,
            nia_layout::ProgramLayoutContext {
                layouts: Some(&layout_query),
                comptimes: Some(&comptime_query),
            },
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct AbiCheckQuery(ModuleId);

impl QueryKey<DriverContext> for AbiCheckQuery {
    type Value = nia_abi_check::AbiCheck;

    fn name() -> &'static str {
        "abi_check"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let defs = db.query(ModuleDefsQuery(self.0));
        let type_lowering = db.query(TypeLoweringQuery(self.0));
        let item_signatures = db.query(ItemSignaturesQuery(self.0));
        let program = db.query(ProgramSignaturesQuery);
        let program_structs = program
            .structs
            .iter()
            .map(|(def_id, signature)| (*def_id, signature.signature.clone()))
            .collect();
        let program_unions = program
            .unions
            .iter()
            .map(|(def_id, signature)| (*def_id, signature.signature.clone()))
            .collect();
        let program_enums = program
            .enums
            .iter()
            .map(|(def_id, signature)| (*def_id, signature.signature.clone()))
            .collect();
        nia_abi_check::check_module_abi_with_program_signatures(
            &defs,
            &type_lowering.interner,
            &item_signatures,
            nia_abi_check::ProgramAbiSignatures {
                structs: &program_structs,
                unions: &program_unions,
                enums: &program_enums,
            },
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct StaticCheckQuery(ModuleId);

impl QueryKey<DriverContext> for StaticCheckQuery {
    type Value = nia_static_check::StaticCheck;

    fn name() -> &'static str {
        "static_check"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let loaded = db.query(LoadedModuleQuery(self.0));
        let defs = db.query(ModuleDefsQuery(self.0));
        let values = db.query(ValueResolutionQuery(self.0));
        let locals = db.query(LocalResolutionQuery(self.0));
        let signatures = db.query(ItemSignaturesQuery(self.0));
        nia_static_check::check_module_static_initializers(
            &loaded.module,
            &defs,
            &values,
            &locals,
            &signatures,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct FlowCheckQuery(ModuleId);

impl QueryKey<DriverContext> for FlowCheckQuery {
    type Value = nia_flow_check::FlowCheck;

    fn name() -> &'static str {
        "flow_check"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let loaded = db.query(LoadedModuleQuery(self.0));
        let type_lowering = db.query(TypeLoweringQuery(self.0));
        let signatures = db.query(ItemSignaturesQuery(self.0));
        nia_flow_check::check_module_flow(&loaded.module, &type_lowering.interner, &signatures)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct BodyCheckQuery(ModuleId);

impl QueryKey<DriverContext> for BodyCheckQuery {
    type Value = nia_body_check::BodyCheck;

    fn name() -> &'static str {
        "body_check"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let loaded = db.query(LoadedModuleQuery(self.0));
        let all_modules = db.query(AllModulesQuery);
        let defs = db.query(ModuleDefsQuery(self.0));
        let all_defs = db.query(DefsByModuleQuery);
        let values = db.query(ValueResolutionQuery(self.0));
        let locals = db.query(LocalResolutionQuery(self.0));
        let lowered = db.query(TypeLoweringQuery(self.0));
        let signatures = db.query(ItemSignaturesQuery(self.0));
        let normalization = db.query(TypeNormalizationQuery(self.0));
        let comptime = db.query(ComptimeQuery(self.0));
        let layouts = db.query(LayoutsQuery(self.0));
        let extensions = db.query(VisibleExtensionsQuery(self.0));
        let program_signatures = db.query(ProgramSignaturesQuery);
        let program_comptime = db.query(ProgramComptimeQuery);
        nia_body_check::check_module_bodies_with_program_signatures_and_layouts(
            nia_body_check::BodyCheckInput {
                module: &loaded.module,
                all_modules: &all_modules,
                defs: &defs,
                all_defs: &all_defs,
                values: &values,
                locals: &locals,
                lowered: &lowered,
                signatures: &signatures,
                normalization: &normalization,
                comptime: &comptime,
                layouts: &layouts,
                extensions: &extensions.methods,
                extension_interner: Some(&extensions.interner),
                program_signatures: program_signatures.maps(),
                program_comptime: nia_body_check::ProgramComptimeMaps {
                    comptimes: &program_comptime,
                },
            },
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CheckedModuleQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for CheckedModuleQuery {
    type Value = CheckedModule;

    fn name() -> &'static str {
        "checked_module"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let loaded = db.query(LoadedModuleQuery(self.0));
        CheckedModule {
            id: loaded.id,
            path: loaded.path,
            defs: db.query(ModuleDefsQuery(self.0)),
            type_resolution: db.query(TypeResolutionQuery(self.0)),
            type_lowering: db.query(TypeLoweringQuery(self.0)),
            value_resolution: db.query(ValueResolutionQuery(self.0)),
            local_resolution: db.query(LocalResolutionQuery(self.0)),
            item_signatures: db.query(ItemSignaturesQuery(self.0)),
            type_normalization: db.query(TypeNormalizationQuery(self.0)),
            comptime: db.query(ComptimeQuery(self.0)),
            static_check: db.query(StaticCheckQuery(self.0)),
            layouts: db.query(LayoutsQuery(self.0)),
            abi_check: db.query(AbiCheckQuery(self.0)),
            flow_check: db.query(FlowCheckQuery(self.0)),
            body_check: db.query(BodyCheckQuery(self.0)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CheckedModulesQuery;

impl QueryKey<DriverContext> for CheckedModulesQuery {
    type Value = Vec<CheckedModule>;

    fn name() -> &'static str {
        "checked_modules"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        db.query_many(
            db.query(ParseOkModuleIdsQuery)
                .into_iter()
                .map(CheckedModuleQuery),
        )
    }
}
