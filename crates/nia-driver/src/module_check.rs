// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{CheckedModule, LoadedModule, program_signatures::VisibleExtensionsForModule};
use nia_body_check::ProgramSignatureMaps;
use nia_defs::{DefCollection, ModuleUsingScope, PublicSurfaces};
use nia_imports::ImportAliasMap;
use nia_item_signatures::ItemSignatures;
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_type_resolve::TypeResolution;

pub(crate) struct CheckLoadedModuleInput<'a> {
    pub(crate) loaded_module: &'a LoadedModule,
    pub(crate) defs: DefCollection,
    pub(crate) imports: &'a ImportAliasMap,
    pub(crate) all_modules: &'a [nia_ast::Module],
    pub(crate) all_defs: &'a [DefCollection],
    pub(crate) extensions: VisibleExtensionsForModule,
    pub(crate) type_resolution: TypeResolution,
    pub(crate) type_lowering: TypeLowering,
    pub(crate) item_signatures: ItemSignatures,
    pub(crate) type_normalization: TypeNormalization,
    pub(crate) public_surfaces: &'a PublicSurfaces,
    pub(crate) using_scope: &'a ModuleUsingScope,
    pub(crate) program_signatures: ProgramSignatureMaps<'a>,
}

pub(crate) fn check_loaded_module(input: CheckLoadedModuleInput<'_>) -> CheckedModule {
    let CheckLoadedModuleInput {
        loaded_module,
        defs,
        imports,
        all_modules,
        all_defs,
        extensions,
        type_resolution,
        type_lowering,
        item_signatures,
        type_normalization,
        public_surfaces,
        using_scope,
        program_signatures,
    } = input;
    let value_resolution = nia_value_resolve::resolve_module_values_with_context(
        &loaded_module.module,
        &defs,
        imports,
        all_defs,
        public_surfaces,
        using_scope,
    );
    let local_resolution =
        nia_local_resolve::resolve_module_locals(&loaded_module.module, &defs, &value_resolution);
    let comptime = nia_comptime_check::check_module_comptime(nia_comptime_check::ComptimeInput {
        module: &loaded_module.module,
        all_modules,
        defs: &defs,
        all_defs,
        values: &value_resolution,
        locals: &local_resolution,
        signatures: &item_signatures,
        interner: &type_normalization.interner,
        const_exprs: &type_lowering.const_exprs,
    });
    let layouts = nia_layout::compute_layouts_with_normalized_types(
        &defs,
        &type_normalization.interner,
        &item_signatures,
        &type_normalization.normalized,
        &comptime,
        nia_layout::TargetDataLayout::LP64,
    );
    let program_structs = program_signatures
        .structs
        .iter()
        .map(|(def_id, signature)| (*def_id, signature.signature.clone()))
        .collect();
    let program_unions = program_signatures
        .unions
        .iter()
        .map(|(def_id, signature)| (*def_id, signature.signature.clone()))
        .collect();
    let program_enums = program_signatures
        .enums
        .iter()
        .map(|(def_id, signature)| (*def_id, signature.signature.clone()))
        .collect();
    let abi_check = nia_abi_check::check_module_abi_with_program_signatures(
        &defs,
        &type_lowering.interner,
        &item_signatures,
        nia_abi_check::ProgramAbiSignatures {
            structs: &program_structs,
            unions: &program_unions,
            enums: &program_enums,
        },
    );
    let static_check = nia_static_check::check_module_static_initializers(
        &loaded_module.module,
        &defs,
        &value_resolution,
        &local_resolution,
        &item_signatures,
    );
    let flow_check = nia_flow_check::check_module_flow(
        &loaded_module.module,
        &type_lowering.interner,
        &item_signatures,
    );
    let body_check = nia_body_check::check_module_bodies_with_program_signatures_and_layouts(
        nia_body_check::BodyCheckInput {
            module: &loaded_module.module,
            all_modules,
            defs: &defs,
            all_defs,
            values: &value_resolution,
            locals: &local_resolution,
            lowered: &type_lowering,
            signatures: &item_signatures,
            normalization: &type_normalization,
            comptime: &comptime,
            layouts: &layouts,
            extensions: &extensions.methods,
            extension_interner: Some(&extensions.interner),
            program_signatures,
        },
    );

    CheckedModule {
        id: loaded_module.id,
        path: loaded_module.path.clone(),
        defs,
        type_resolution,
        type_lowering,
        value_resolution,
        local_resolution,
        item_signatures,
        type_normalization,
        comptime,
        static_check,
        layouts,
        abi_check,
        flow_check,
        body_check,
    }
}
