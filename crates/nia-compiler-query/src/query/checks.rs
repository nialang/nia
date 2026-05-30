// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ComptimeQuery(pub(super) ModuleId);

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
pub(super) struct LayoutsQuery(pub(super) ModuleId);

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
pub(super) struct AbiCheckQuery(pub(super) ModuleId);

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
pub(super) struct StaticCheckQuery(pub(super) ModuleId);

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
pub(super) struct FlowCheckQuery(pub(super) ModuleId);

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
pub(super) struct BodyCheckQuery(pub(super) ModuleId);

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
