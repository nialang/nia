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

pub(super) struct BackendLoweringModuleInputsInput<'a> {
    pub(super) checked_modules: &'a [CheckedModule],
    pub(super) module_asts: &'a [nia_ast::Module],
    pub(super) visible_extensions: &'a [VisibleExtensionsValue],
    pub(super) function_bodies: &'a [LoweredFunctionBodies],
    pub(super) extension_methods: &'a ExtensionMethodsQueryValue,
    pub(super) program_defs: &'a HashMap<ModuleId, DefCollection>,
    pub(super) program_signatures: &'a ProgramSignatures,
    pub(super) indexes: &'a BackendLoweringIndexes<'a>,
}

pub(super) fn build_backend_lowering_module_inputs<'a>(
    input: BackendLoweringModuleInputsInput<'a>,
) -> Vec<BackendLowerModuleInput<'a>> {
    input
        .checked_modules
        .iter()
        .zip(input.module_asts.iter())
        .zip(input.visible_extensions.iter())
        .zip(input.function_bodies.iter())
        .map(
            |(((checked_module, module_ast), visible_extensions), function_bodies)| {
                BackendLowerModuleInput {
                    module_id: checked_module.id,
                    module_name: checked_module.path.as_str().to_string(),
                    module: module_ast,
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
                    program_comptime: &input.indexes.program_comptime,
                    layouts: &checked_module.layouts,
                    function_bodies: &function_bodies.bodies,
                    roots: backend_function_roots(),
                    program_function_bodies: &input.indexes.program_function_bodies,
                    extension_interner: Some(&visible_extensions.interner),
                    program_extension_methods: &input.extension_methods.methods,
                    program_extensions: &input.indexes.program_extensions,
                    program_defs: input.program_defs,
                    program_type_interners: &input.indexes.program_type_interners,
                    program_functions: &input.program_signatures.functions,
                    program_structs: &input.program_signatures.structs,
                    program_unions: &input.program_signatures.unions,
                    program_enums: &input.program_signatures.enums,
                    program_traits: &input.program_signatures.traits,
                    trait_impls: &input.program_signatures.trait_impls,
                }
            },
        )
        .collect()
}

fn backend_function_roots() -> nia_backend_lower::BackendFunctionRoots {
    nia_backend_lower::BackendFunctionRoots::FunctionBodies
}
