// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::LoadedModule;
use nia_body_check::{
    ProgramEnumSignature, ProgramFunctionSignature, ProgramGlobalSignature, ProgramStructSignature,
    ProgramUnionSignature, import_type_into,
};
use nia_defs::{
    DefCollection, ExtensionMethod, ExtensionMethods, VisibleExtensionMethod,
    VisibleExtensionMethods,
};
use nia_diagnostic::Diagnostic;
use nia_ids::GlobalDefId;
use nia_item_signatures::ItemSignatures;
use nia_ty::{PrimitiveTy, TyInterner, TyKind};
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
            if !is_extendable_target(&lowering.interner, target_ty) {
                diagnostics.push(Diagnostic::error(
                    extend.target.span,
                    "extend target must be an extendable value type",
                ));
                continue;
            }
            let is_nominal_target = matches!(
                lowering.interner.get(target_ty),
                Some(TyKind::Nominal { .. })
            );
            for method in &extend.methods {
                let Some(method_id) = defs.def_spans.get(method.function.span) else {
                    continue;
                };
                if !is_nominal_target
                    && method
                        .function
                        .params
                        .first()
                        .is_none_or(|param| param.receiver.is_none())
                {
                    diagnostics.push(Diagnostic::error(
                        method.function.span,
                        "associated functions are not supported for non-nominal extend targets",
                    ));
                    continue;
                }
                extensions.insert(
                    module.id,
                    ExtensionMethod {
                        def_id: GlobalDefId {
                            module_id: module.id,
                            def_id: method_id,
                        },
                        target_ty,
                        visibility: method.vis,
                    },
                );
            }
        }
    }
    (extensions, diagnostics)
}

fn is_extendable_target(interner: &TyInterner, ty: nia_ids::TyId) -> bool {
    match interner.get(ty) {
        Some(TyKind::Error) | None => false,
        Some(TyKind::Primitive(PrimitiveTy::Never)) => false,
        Some(TyKind::Array { len, .. }) => !matches!(len, nia_ty::ArrayLenTy::Infer),
        Some(
            TyKind::Primitive(_)
            | TyKind::Pointer { .. }
            | TyKind::Slice { .. }
            | TyKind::FunctionPointer { .. }
            | TyKind::Nominal { .. }
            | TyKind::GenericParam(_),
        ) => true,
    }
}

pub(crate) fn visible_extensions_for_module(
    module_id: nia_ids::ModuleId,
    imports: &nia_imports::ImportAliasMap,
    defs_by_module: &[DefCollection],
    lowerings: &[TypeLowering],
    normalizations: &[TypeNormalization],
    extensions: &ExtensionMethods,
) -> VisibleExtensionMethods {
    let imported_modules = imports
        .module_aliases(module_id)
        .into_iter()
        .flat_map(|aliases| aliases.values())
        .map(|alias| alias.target)
        .collect::<Vec<_>>();
    let Some(current_lowering) = lowerings
        .iter()
        .zip(defs_by_module)
        .find(|(_, defs)| defs.module_id == module_id)
        .map(|(lowering, _)| lowering)
    else {
        return VisibleExtensionMethods::default();
    };
    let mut target_interner = current_lowering.interner.clone();
    let mut visible = VisibleExtensionMethods::default();
    for method in extensions.visible_methods(module_id, imported_modules) {
        let Some(method_defs) = defs_by_module
            .iter()
            .find(|defs| defs.module_id == method.def_id.module_id)
        else {
            continue;
        };
        let Some(method_def) = method_defs.defs.get(method.def_id.def_id) else {
            continue;
        };
        let Some(method_module_index) = defs_by_module
            .iter()
            .position(|defs| defs.module_id == method.def_id.module_id)
        else {
            continue;
        };
        let target_ty = normalizations[method_module_index].normalize(method.target_ty);
        let target_ty = import_type_into(
            &mut target_interner,
            &lowerings[method_module_index].interner,
            target_ty,
        );
        visible.insert(
            target_ty,
            VisibleExtensionMethod {
                name: method_def.name.clone(),
                def_id: method.def_id,
            },
        );
    }
    visible
}
