// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) struct BackendLoweringIndexes<'a> {
    pub(super) program_extensions: HashMap<
        ModuleId,
        (
            &'a nia_defs::VisibleExtensionMethods,
            &'a nia_ty::TyInterner,
        ),
    >,
    pub(super) program_type_interners: HashMap<ModuleId, &'a nia_ty::TyInterner>,
    pub(super) program_function_bodies: HashMap<GlobalDefId, nia_function_ir::FunctionBody>,
    pub(super) program_comptime: HashMap<ModuleId, &'a ComptimeCheck>,
}

pub(super) fn build_backend_lowering_indexes<'a>(
    checked_modules: &'a [CheckedModule],
    visible_extensions: &'a [VisibleExtensionsValue],
    function_bodies: &'a [LoweredFunctionBodies],
) -> BackendLoweringIndexes<'a> {
    let program_extensions = checked_modules
        .iter()
        .zip(visible_extensions.iter())
        .map(|(checked_module, visible_extensions)| {
            (
                checked_module.id,
                (&visible_extensions.methods, &visible_extensions.interner),
            )
        })
        .collect::<HashMap<_, _>>();
    let program_type_interners = checked_modules
        .iter()
        .zip(function_bodies.iter())
        .map(|(checked_module, lowered)| (checked_module.id, &lowered.interner))
        .collect::<HashMap<_, _>>();
    let program_function_bodies = function_bodies
        .iter()
        .flat_map(|lowered| {
            lowered
                .bodies
                .iter()
                .map(|(def_id, body)| (*def_id, body.clone()))
        })
        .collect::<HashMap<_, _>>();
    let program_comptime = checked_modules
        .iter()
        .map(|checked_module| (checked_module.id, &checked_module.comptime))
        .collect::<HashMap<_, _>>();

    BackendLoweringIndexes {
        program_extensions,
        program_type_interners,
        program_function_bodies,
        program_comptime,
    }
}

pub(super) fn build_backend_lowering_module_inputs<'a>(
    checked_modules: &'a [CheckedModule],
    loaded_modules: &'a [LoadedModule],
    visible_extensions: &'a [VisibleExtensionsValue],
    function_bodies: &'a [LoweredFunctionBodies],
    extension_methods: &'a ExtensionMethodsQueryValue,
    program_defs: &'a HashMap<ModuleId, DefCollection>,
    program_signatures: &'a ProgramSignatures,
    indexes: &'a BackendLoweringIndexes<'a>,
) -> Vec<BackendLowerModuleInput<'a>> {
    checked_modules
        .iter()
        .zip(loaded_modules.iter())
        .zip(visible_extensions.iter())
        .zip(function_bodies.iter())
        .map(
            |(((checked_module, loaded_module), visible_extensions), function_bodies)| {
                BackendLowerModuleInput {
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
                    body_ir: &checked_module.body_ir,
                    function_interner: &function_bodies.interner,
                    semantic_facts: &checked_module.semantic_facts,
                    comptime: &checked_module.comptime,
                    program_comptime: &indexes.program_comptime,
                    layouts: &checked_module.layouts,
                    function_bodies: &function_bodies.bodies,
                    roots: backend_function_roots(),
                    program_function_bodies: &indexes.program_function_bodies,
                    extension_interner: Some(&visible_extensions.interner),
                    program_extension_methods: &extension_methods.methods,
                    program_extensions: &indexes.program_extensions,
                    program_defs,
                    program_type_interners: &indexes.program_type_interners,
                    program_functions: &program_signatures.functions,
                    program_structs: &program_signatures.structs,
                    program_unions: &program_signatures.unions,
                    program_enums: &program_signatures.enums,
                    program_traits: &program_signatures.traits,
                    trait_impls: &program_signatures.trait_impls,
                }
            },
        )
        .collect()
}

fn backend_function_roots() -> nia_backend_lower::BackendFunctionRoots {
    nia_backend_lower::BackendFunctionRoots::FunctionBodies
}
