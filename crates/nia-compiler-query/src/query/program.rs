// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct MonomorphizationQuery;

impl QueryKey<DriverContext> for MonomorphizationQuery {
    type Value = nia_monomorphize::Monomorphization;

    fn name() -> &'static str {
        "monomorphization"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let checked_modules = db.query(CheckedModulesQuery);
        nia_monomorphize::collect_monomorphizations(
            &checked_modules
                .iter()
                .map(|module| MonomorphizeModuleInput {
                    module_id: module.id,
                    defs: &module.defs,
                    interner: &module.body_check.ir.interner,
                    comptime: &module.comptime,
                    instantiations: &module.body_check.ir.generic_instantiations,
                })
                .collect::<Vec<_>>(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct BackendLoweringQuery;

impl QueryKey<DriverContext> for BackendLoweringQuery {
    type Value = nia_backend_lower::BackendLowering;

    fn name() -> &'static str {
        "backend_lowering"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let checked_modules = db.query(CheckedModulesQuery);
        let monomorphization = db.query(MonomorphizationQuery);
        let loaded_modules = checked_modules
            .iter()
            .map(|checked_module| db.query(LoadedModuleQuery(checked_module.id)))
            .collect::<Vec<_>>();
        let visible_extensions = checked_modules
            .iter()
            .map(|checked_module| db.query(VisibleExtensionsQuery(checked_module.id)))
            .collect::<Vec<_>>();
        let inputs = checked_modules
            .iter()
            .zip(loaded_modules.iter())
            .zip(visible_extensions.iter())
            .map(
                |((checked_module, loaded_module), visible_extensions)| BackendLowerModuleInput {
                    module_id: checked_module.id,
                    module_name: checked_module.path.as_str().to_string(),
                    module: &loaded_module.module,
                    defs: &checked_module.defs,
                    extensions: &visible_extensions.methods,
                    values: &checked_module.value_resolution,
                    locals: &checked_module.local_resolution,
                    type_lowering: &checked_module.type_lowering,
                    signatures: &checked_module.item_signatures,
                    type_normalization: &checked_module.type_normalization,
                    body_check: &checked_module.body_check,
                    comptime: &checked_module.comptime,
                    layouts: &checked_module.layouts,
                    extension_interner: Some(&visible_extensions.interner),
                },
            )
            .collect::<Vec<_>>();
        nia_backend_lower::lower_backend_program(&inputs, &monomorphization)
    }
}
