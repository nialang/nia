// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::LoadedModule;
use nia_body_check::{
    ProgramEnumSignature, ProgramFunctionSignature, ProgramGlobalSignature, ProgramStructSignature,
    ProgramUnionSignature,
};
use nia_defs::{
    DefCollection, ExtensionMethod, ExtensionMethods, VisibleExtensionMethod,
    VisibleExtensionMethods,
};
use nia_diagnostic::Diagnostic;
use nia_ids::GlobalDefId;
use nia_item_signatures::ItemSignatures;
use nia_ty::TyKind;
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;

pub(crate) fn collect_program_functions(
    modules: &[LoadedModule],
    lowerings: &[TypeLowering],
    signatures: &[ItemSignatures],
) -> HashMap<GlobalDefId, ProgramFunctionSignature> {
    let mut functions = HashMap::new();
    for ((module, lowering), signatures) in modules.iter().zip(lowerings).zip(signatures) {
        for (def_id, signature) in &signatures.functions {
            functions.insert(
                GlobalDefId {
                    module_id: module.id,
                    def_id: *def_id,
                },
                ProgramFunctionSignature {
                    signature: signature.clone(),
                    interner: lowering.interner.clone(),
                },
            );
        }
    }
    functions
}

pub(crate) fn collect_program_globals(
    modules: &[LoadedModule],
    lowerings: &[TypeLowering],
    signatures: &[ItemSignatures],
) -> HashMap<GlobalDefId, ProgramGlobalSignature> {
    let mut globals = HashMap::new();
    for ((module, lowering), signatures) in modules.iter().zip(lowerings).zip(signatures) {
        for (def_id, signature) in &signatures.globals {
            globals.insert(
                GlobalDefId {
                    module_id: module.id,
                    def_id: *def_id,
                },
                ProgramGlobalSignature {
                    signature: signature.clone(),
                    interner: lowering.interner.clone(),
                },
            );
        }
    }
    globals
}

pub(crate) fn collect_program_structs(
    modules: &[LoadedModule],
    lowerings: &[TypeLowering],
    signatures: &[ItemSignatures],
) -> HashMap<GlobalDefId, ProgramStructSignature> {
    let mut structs = HashMap::new();
    for ((module, lowering), signatures) in modules.iter().zip(lowerings).zip(signatures) {
        for (def_id, signature) in &signatures.structs {
            structs.insert(
                GlobalDefId {
                    module_id: module.id,
                    def_id: *def_id,
                },
                ProgramStructSignature {
                    signature: signature.clone(),
                    interner: lowering.interner.clone(),
                },
            );
        }
    }
    structs
}

pub(crate) fn collect_program_unions(
    modules: &[LoadedModule],
    lowerings: &[TypeLowering],
    signatures: &[ItemSignatures],
) -> HashMap<GlobalDefId, ProgramUnionSignature> {
    let mut unions = HashMap::new();
    for ((module, lowering), signatures) in modules.iter().zip(lowerings).zip(signatures) {
        for (def_id, signature) in &signatures.unions {
            unions.insert(
                GlobalDefId {
                    module_id: module.id,
                    def_id: *def_id,
                },
                ProgramUnionSignature {
                    signature: signature.clone(),
                    interner: lowering.interner.clone(),
                },
            );
        }
    }
    unions
}

pub(crate) fn collect_program_enums(
    modules: &[LoadedModule],
    lowerings: &[TypeLowering],
    signatures: &[ItemSignatures],
) -> HashMap<GlobalDefId, ProgramEnumSignature> {
    let mut enums = HashMap::new();
    for ((module, lowering), signatures) in modules.iter().zip(lowerings).zip(signatures) {
        for (def_id, signature) in &signatures.enums {
            enums.insert(
                GlobalDefId {
                    module_id: module.id,
                    def_id: *def_id,
                },
                ProgramEnumSignature {
                    signature: signature.clone(),
                    interner: lowering.interner.clone(),
                },
            );
        }
    }
    enums
}

pub(crate) fn collect_extension_methods(
    modules: &[LoadedModule],
    defs_by_module: &[DefCollection],
    lowerings: &[TypeLowering],
    normalizations: &[TypeNormalization],
) -> (ExtensionMethods, Vec<Diagnostic>) {
    let mut extensions = ExtensionMethods::default();
    let mut diagnostics = Vec::new();
    for (((module, defs), lowering), normalization) in modules
        .iter()
        .zip(defs_by_module)
        .zip(lowerings)
        .zip(normalizations)
    {
        for item in &module.module.items {
            let nia_ast::ItemKind::Extend(extend) = &item.kind else {
                continue;
            };
            let Some(target_ty) = lowering.type_uses.get(&extend.target.span).copied() else {
                diagnostics.push(Diagnostic::error(
                    extend.target.span,
                    "extend target must resolve to a nominal type",
                ));
                continue;
            };
            let target_ty = normalization.normalize(target_ty);
            let Some(TyKind::Nominal {
                def_id: target,
                args: target_args,
            }) = lowering.interner.get(target_ty)
            else {
                diagnostics.push(Diagnostic::error(
                    extend.target.span,
                    "extend target must be a nominal type",
                ));
                continue;
            };
            for method in &extend.methods {
                let Some(method_id) = defs.def_spans.get(method.function.span) else {
                    continue;
                };
                extensions.insert(
                    module.id,
                    *target,
                    ExtensionMethod {
                        def_id: GlobalDefId {
                            module_id: module.id,
                            def_id: method_id,
                        },
                        target_args: target_args.clone(),
                        visibility: method.vis,
                    },
                );
            }
        }
    }
    (extensions, diagnostics)
}

pub(crate) fn visible_extensions_for_module(
    module_id: nia_ids::ModuleId,
    imports: &nia_imports::ImportAliasMap,
    defs_by_module: &[DefCollection],
    extensions: &ExtensionMethods,
) -> VisibleExtensionMethods {
    let imported_modules = imports
        .module_aliases(module_id)
        .into_iter()
        .flat_map(|aliases| aliases.values())
        .map(|alias| alias.target)
        .collect::<Vec<_>>();
    let mut visible = VisibleExtensionMethods::default();
    for target_defs in defs_by_module {
        for (def_id, def) in target_defs.defs.iter() {
            let target = GlobalDefId {
                module_id: target_defs.module_id,
                def_id,
            };
            for method in extensions.visible_methods(module_id, imported_modules.clone(), target) {
                let Some(method_defs) = defs_by_module
                    .iter()
                    .find(|defs| defs.module_id == method.def_id.module_id)
                else {
                    continue;
                };
                let Some(method_def) = method_defs.defs.get(method.def_id.def_id) else {
                    continue;
                };
                if !matches!(
                    def.kind,
                    nia_defs::DefKind::Struct | nia_defs::DefKind::Union
                ) {
                    continue;
                }
                visible.insert(
                    target,
                    method.target_args.clone(),
                    VisibleExtensionMethod {
                        name: method_def.name.clone(),
                        def_id: method.def_id,
                    },
                );
            }
        }
    }
    visible
}
