// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    CheckedModule, CheckedProgram, LoadedModule, ProgramDiagnostic, load_program,
    load_program_with_map,
    module_check::{CheckLoadedModuleInput, check_loaded_module},
    module_diagnostics,
    program_signatures::{
        collect_extension_methods, collect_program_enums, collect_program_functions,
        collect_program_globals, collect_program_structs, collect_program_unions,
        visible_extensions_for_module,
    },
    public_surface::compute_public_surfaces,
};
use nia_backend_lower::BackendLowerModuleInput;
use nia_defs::DefCollection;
use nia_diagnostic::Diagnostic;
use nia_imports::ModuleMap;
use nia_item_signatures::ItemSignatures;
use nia_monomorphize::MonomorphizeModuleInput;
use nia_span::Span;
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_type_resolve::TypeResolution;

pub fn check_program(root_path: impl Into<String>) -> CheckedProgram {
    check_program_with_loaded(load_program(root_path))
}

pub fn check_program_with_map(
    root_path: impl Into<String>,
    module_map: ModuleMap,
) -> CheckedProgram {
    check_program_with_loaded(load_program_with_map(root_path, module_map))
}

fn check_program_with_loaded(loaded: crate::LoadedProgram) -> CheckedProgram {
    let mut diagnostics = loaded.diagnostics;
    let mut checked_modules = Vec::new();
    let parse_ok_modules: Vec<LoadedModule> = loaded
        .modules
        .into_iter()
        .filter(|loaded_module| {
            for error in &loaded_module.parse_errors {
                diagnostics.push(ProgramDiagnostic {
                    path: loaded_module.path.clone(),
                    diagnostic: Diagnostic::error(error.span, error.message.clone()),
                });
            }
            loaded_module.parse_errors.is_empty()
        })
        .collect();
    let defs_by_module: Vec<DefCollection> = parse_ok_modules
        .iter()
        .map(|module| nia_defs::collect_module_defs(module.id, &module.module))
        .collect();
    let (public_surfaces, using_scopes, public_surface_diagnostics) =
        compute_public_surfaces(&defs_by_module, &loaded.imports);
    for (module_id, diagnostic) in public_surface_diagnostics {
        let path = parse_ok_modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.path.clone())
            .unwrap_or_else(|| nia_imports::SourcePath::new("<unknown>"));
        diagnostics.push(ProgramDiagnostic { path, diagnostic });
    }
    let _ = (&public_surfaces, &using_scopes);
    let type_resolutions: Vec<TypeResolution> = parse_ok_modules
        .iter()
        .zip(defs_by_module.iter())
        .map(|(module, defs)| {
            let empty_using = nia_defs::ModuleUsingScope::default();
            let using_scope = using_scopes.get(&module.id).unwrap_or(&empty_using);
            nia_type_resolve::resolve_module_types_with_context(
                &module.module,
                defs,
                &loaded.imports,
                &defs_by_module,
                &public_surfaces,
                using_scope,
            )
        })
        .collect();
    let type_lowerings: Vec<TypeLowering> = parse_ok_modules
        .iter()
        .zip(type_resolutions.iter())
        .map(|(module, type_resolution)| {
            nia_type_lower::lower_module_types_with_defs(
                module.id,
                &module.module,
                type_resolution,
                &defs_by_module,
            )
        })
        .collect();
    let item_signatures_by_module: Vec<ItemSignatures> = parse_ok_modules
        .iter()
        .zip(defs_by_module.iter())
        .zip(type_lowerings.iter())
        .map(|((module, defs), type_lowering)| {
            nia_item_signatures::collect_item_signatures(&module.module, defs, type_lowering)
        })
        .collect();
    let type_normalizations: Vec<TypeNormalization> = parse_ok_modules
        .iter()
        .zip(type_lowerings.iter())
        .zip(item_signatures_by_module.iter())
        .map(|((module, type_lowering), item_signatures)| {
            nia_type_normalize::normalize_module_types(
                module.id,
                &type_lowering.interner,
                item_signatures,
            )
        })
        .collect();
    let program_functions = collect_program_functions(
        &parse_ok_modules,
        &type_lowerings,
        &item_signatures_by_module,
    );
    let program_globals = collect_program_globals(
        &parse_ok_modules,
        &type_lowerings,
        &item_signatures_by_module,
    );
    let program_structs = collect_program_structs(
        &parse_ok_modules,
        &type_lowerings,
        &item_signatures_by_module,
    );
    let program_unions = collect_program_unions(
        &parse_ok_modules,
        &type_lowerings,
        &item_signatures_by_module,
    );
    let program_enums = collect_program_enums(
        &parse_ok_modules,
        &type_lowerings,
        &item_signatures_by_module,
    );
    let (extension_methods, extension_diagnostics) = collect_extension_methods(
        &parse_ok_modules,
        &defs_by_module,
        &type_lowerings,
        &type_normalizations,
    );
    let visible_extensions_by_module = parse_ok_modules
        .iter()
        .map(|module| {
            visible_extensions_for_module(
                module.id,
                &loaded.imports,
                &defs_by_module,
                &type_normalizations,
                &extension_methods,
            )
        })
        .collect::<Vec<_>>();
    diagnostics.extend(extension_diagnostics.iter().cloned().map(|diagnostic| {
        ProgramDiagnostic {
            path: parse_ok_modules
                .first()
                .map(|module| module.path.clone())
                .unwrap_or_else(|| nia_imports::SourcePath::new("<unknown>")),
            diagnostic,
        }
    }));

    for (
        (
            ((((loaded_module, defs), type_resolution), type_lowering), item_signatures),
            type_normalization,
        ),
        visible_extensions,
    ) in parse_ok_modules
        .iter()
        .zip(defs_by_module.iter())
        .zip(type_resolutions)
        .zip(type_lowerings)
        .zip(item_signatures_by_module)
        .zip(type_normalizations)
        .zip(visible_extensions_by_module.iter())
    {
        let empty_using = nia_defs::ModuleUsingScope::default();
        let using_scope = using_scopes.get(&loaded_module.id).unwrap_or(&empty_using);
        let checked = check_loaded_module(CheckLoadedModuleInput {
            loaded_module,
            defs: defs.clone(),
            imports: &loaded.imports,
            all_defs: &defs_by_module,
            extensions: visible_extensions.clone(),
            type_resolution,
            type_lowering,
            item_signatures,
            type_normalization,
            public_surfaces: &public_surfaces,
            using_scope,
            program_signatures: nia_body_check::ProgramSignatureMaps {
                functions: &program_functions,
                globals: &program_globals,
                structs: &program_structs,
                unions: &program_unions,
                enums: &program_enums,
            },
        });
        diagnostics.extend(module_diagnostics(
            &loaded_module.path,
            &checked.defs.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &loaded_module.path,
            &checked.type_resolution.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &loaded_module.path,
            &checked.type_lowering.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &loaded_module.path,
            &checked.value_resolution.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &loaded_module.path,
            &checked.local_resolution.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &loaded_module.path,
            &checked.item_signatures.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &loaded_module.path,
            &checked.type_normalization.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &loaded_module.path,
            &checked.const_eval.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &loaded_module.path,
            &checked.static_check.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &loaded_module.path,
            &checked.layouts.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &loaded_module.path,
            &checked.abi_check.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &loaded_module.path,
            &checked.flow_check.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &loaded_module.path,
            &checked.body_check.diagnostics,
        ));
        checked_modules.push(checked);
    }

    let monomorphization = nia_monomorphize::collect_monomorphizations(
        &checked_modules
            .iter()
            .map(|module| MonomorphizeModuleInput {
                module_id: module.id,
                defs: &module.defs,
                interner: &module.body_check.interner,
                instantiations: &module.body_check.generic_instantiations,
            })
            .collect::<Vec<_>>(),
    );
    diagnostics.extend(
        monomorphization
            .diagnostics
            .iter()
            .cloned()
            .map(|diagnostic| ProgramDiagnostic {
                path: path_for_diagnostic_span(&checked_modules, diagnostic.span),
                diagnostic,
            }),
    );
    let backend_lowering = nia_backend_lower::lower_backend_program(
        &checked_modules
            .iter()
            .zip(visible_extensions_by_module.iter())
            .filter_map(|(checked_module, visible_extensions)| {
                let loaded_module = parse_ok_modules
                    .iter()
                    .find(|module| module.id == checked_module.id)?;
                Some(BackendLowerModuleInput {
                    module_id: checked_module.id,
                    module_name: checked_module.path.as_str().to_string(),
                    module: &loaded_module.module,
                    defs: &checked_module.defs,
                    all_defs: &defs_by_module,
                    extensions: &visible_extensions.methods,
                    values: &checked_module.value_resolution,
                    locals: &checked_module.local_resolution,
                    type_lowering: &checked_module.type_lowering,
                    signatures: &checked_module.item_signatures,
                    type_normalization: &checked_module.type_normalization,
                    body_check: &checked_module.body_check,
                    const_eval: &checked_module.const_eval,
                    layouts: &checked_module.layouts,
                    extension_interner: Some(&visible_extensions.interner),
                })
            })
            .collect::<Vec<_>>(),
        &monomorphization,
    );
    diagnostics.extend(
        backend_lowering
            .diagnostics
            .iter()
            .cloned()
            .map(|diagnostic| ProgramDiagnostic {
                path: path_for_diagnostic_span(&checked_modules, diagnostic.span),
                diagnostic,
            }),
    );

    CheckedProgram {
        graph: loaded.graph,
        imports: loaded.imports,
        modules: checked_modules,
        monomorphization,
        backend_lowering,
        diagnostics,
    }
}

fn path_for_diagnostic_span(modules: &[CheckedModule], span: Span) -> nia_imports::SourcePath {
    modules
        .iter()
        .find(|module| {
            module
                .body_check
                .generic_instantiations
                .iter()
                .any(|instantiation| instantiation.span == span)
        })
        .map(|module| module.path.clone())
        .unwrap_or_else(|| {
            modules
                .first()
                .map(|module| module.path.clone())
                .unwrap_or_else(|| nia_imports::SourcePath::new("<unknown>"))
        })
}
