// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet, VecDeque};

use crate::LoadedModule;
use nia_defs::{
    AssociatedTypeBindingSignature, DefCollection, ExtensionAssociatedValue,
    ExtensionAssociatedValues, ExtensionMethod, ExtensionMethods, PublicNamespace, PublicSurfaces,
    VisibleExtensionAssociatedValue, VisibleExtensionMethod, VisibleExtensionMethods,
    WhereBoundSignature, WherePredicateSignature,
};
use nia_diagnostic::Diagnostic;
use nia_ids::{BuiltinReceiverKind, BuiltinTrait, BuiltinTraitMethod, GlobalDefId, InternedTyId};
use nia_item_signatures::{
    FunctionSignature, ItemSignatures, ParamSignature, ProgramComptimeSignature,
    ProgramEnumSignature, ProgramFunctionSignature, ProgramGlobalSignature, ProgramStructSignature,
    ProgramTraitImplSignature, ProgramTraitSignature, ProgramTypeAliasSignature,
    ProgramUnionSignature, TraitSignature,
};
use nia_trait_solve::IntrinsicOverlap;
use nia_ty::{
    ArrayLenTy, PrimitiveTy, TraitId, TyInterner, TyKind, TypeEquivalence, import_type_into,
};
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;

pub(crate) struct ModuleSignatureInput<'a> {
    pub(crate) module_id: nia_ids::ModuleId,
    pub(crate) defs: &'a DefCollection,
    pub(crate) lowering: &'a TypeLowering,
    pub(crate) signatures: &'a ItemSignatures,
}

pub(crate) struct ExtensionModuleInput<'a> {
    pub(crate) module: &'a LoadedModule,
    pub(crate) defs: &'a DefCollection,
    pub(crate) lowering: &'a TypeLowering,
    pub(crate) signatures: &'a ItemSignatures,
    pub(crate) normalization: &'a TypeNormalization,
}

fn lowered_type(module: &ExtensionModuleInput<'_>, ty: &nia_ast::TypeRef) -> Option<InternedTyId> {
    module.lowering.node_type_uses.get(&ty.node_key).copied()
}

fn where_predicates(
    module: &ExtensionModuleInput<'_>,
    clause: &nia_ast::WhereClause,
) -> Vec<WherePredicateSignature> {
    clause
        .predicates
        .iter()
        .map(|predicate| WherePredicateSignature {
            ty: lowered_type(module, &predicate.ty)
                .unwrap_or_else(|| module.lowering.interner.error()),
            bounds: predicate
                .bounds
                .iter()
                .map(|bound| WhereBoundSignature {
                    trait_ty: lowered_type(module, bound)
                        .unwrap_or_else(|| module.lowering.interner.error()),
                    associated_type_bindings: associated_type_bindings(module, bound),
                    span: bound.span,
                })
                .collect(),
            span: predicate.span,
        })
        .collect()
}

fn associated_type_bindings(
    module: &ExtensionModuleInput<'_>,
    bound: &nia_ast::TypeRef,
) -> Vec<AssociatedTypeBindingSignature> {
    let nia_ast::TypeKind::Path { segments } = &bound.kind else {
        return Vec::new();
    };
    let Some(segment) = segments.last() else {
        return Vec::new();
    };
    segment
        .args
        .iter()
        .filter_map(|arg| match arg {
            nia_ast::TypeArg::AssocBinding { key, ty, span } => {
                let name = match key {
                    nia_ast::AssocBindingKey::Name(name) => name.clone(),
                    nia_ast::AssocBindingKey::Projection(projection) => {
                        let nia_ast::TypeKind::Projection { name, .. } = &projection.kind else {
                            return None;
                        };
                        name.clone()
                    }
                };
                Some(AssociatedTypeBindingSignature {
                    name,
                    ty: lowered_type(module, ty)
                        .unwrap_or_else(|| module.lowering.interner.error()),
                    span: *span,
                })
            }
            nia_ast::TypeArg::Type(_) | nia_ast::TypeArg::Const(_) => None,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VisibleExtensionsForModule {
    pub(crate) methods: VisibleExtensionMethods,
    pub(crate) interner: TyInterner,
}

pub(crate) fn collect_program_functions(
    modules: &[ModuleSignatureInput<'_>],
) -> HashMap<GlobalDefId, ProgramFunctionSignature> {
    let mut functions = HashMap::new();
    for module in modules {
        for (def_id, signature) in &module.signatures.functions {
            functions.insert(
                GlobalDefId {
                    module_id: module.module_id,
                    def_id: *def_id,
                },
                ProgramFunctionSignature {
                    name: module
                        .defs
                        .defs
                        .get(*def_id)
                        .map(|def| def.name.clone())
                        .unwrap_or_else(|| format!("def{}", def_id.0)),
                    signature: signature.clone(),
                    interner: module.lowering.interner.clone(),
                },
            );
        }
    }
    functions
}

pub(crate) fn collect_program_globals(
    modules: &[ModuleSignatureInput<'_>],
) -> HashMap<GlobalDefId, ProgramGlobalSignature> {
    let mut globals = HashMap::new();
    for module in modules {
        for (def_id, signature) in &module.signatures.globals {
            globals.insert(
                GlobalDefId {
                    module_id: module.module_id,
                    def_id: *def_id,
                },
                ProgramGlobalSignature {
                    signature: signature.clone(),
                    interner: module.lowering.interner.clone(),
                },
            );
        }
    }
    globals
}

pub(crate) fn collect_program_comptimes(
    modules: &[ModuleSignatureInput<'_>],
) -> HashMap<GlobalDefId, ProgramComptimeSignature> {
    let mut comptimes = HashMap::new();
    for module in modules {
        for (def_id, signature) in &module.signatures.comptimes {
            comptimes.insert(
                GlobalDefId {
                    module_id: module.module_id,
                    def_id: *def_id,
                },
                ProgramComptimeSignature {
                    signature: signature.clone(),
                    interner: module.lowering.interner.clone(),
                },
            );
        }
    }
    comptimes
}

pub(crate) fn collect_program_structs(
    modules: &[ModuleSignatureInput<'_>],
) -> HashMap<GlobalDefId, ProgramStructSignature> {
    let mut structs = HashMap::new();
    for module in modules {
        for (def_id, signature) in &module.signatures.structs {
            structs.insert(
                GlobalDefId {
                    module_id: module.module_id,
                    def_id: *def_id,
                },
                ProgramStructSignature {
                    signature: signature.clone(),
                    interner: module.lowering.interner.clone(),
                },
            );
        }
    }
    structs
}

pub(crate) fn collect_program_unions(
    modules: &[ModuleSignatureInput<'_>],
) -> HashMap<GlobalDefId, ProgramUnionSignature> {
    let mut unions = HashMap::new();
    for module in modules {
        for (def_id, signature) in &module.signatures.unions {
            unions.insert(
                GlobalDefId {
                    module_id: module.module_id,
                    def_id: *def_id,
                },
                ProgramUnionSignature {
                    signature: signature.clone(),
                    interner: module.lowering.interner.clone(),
                },
            );
        }
    }
    unions
}

pub(crate) fn collect_program_enums(
    modules: &[ModuleSignatureInput<'_>],
) -> HashMap<GlobalDefId, ProgramEnumSignature> {
    let mut enums = HashMap::new();
    for module in modules {
        for (def_id, signature) in &module.signatures.enums {
            enums.insert(
                GlobalDefId {
                    module_id: module.module_id,
                    def_id: *def_id,
                },
                ProgramEnumSignature {
                    signature: signature.clone(),
                    interner: module.lowering.interner.clone(),
                },
            );
        }
    }
    enums
}

pub(crate) fn collect_program_traits(
    modules: &[ModuleSignatureInput<'_>],
) -> HashMap<GlobalDefId, ProgramTraitSignature> {
    let mut traits = HashMap::new();
    for module in modules {
        for (def_id, signature) in &module.signatures.traits {
            traits.insert(
                GlobalDefId {
                    module_id: module.module_id,
                    def_id: *def_id,
                },
                ProgramTraitSignature {
                    signature: signature.clone(),
                    interner: module.lowering.interner.clone(),
                },
            );
        }
    }
    traits
}

pub(crate) fn collect_program_type_aliases(
    modules: &[ModuleSignatureInput<'_>],
) -> HashMap<GlobalDefId, ProgramTypeAliasSignature> {
    let mut type_aliases = HashMap::new();
    for module in modules {
        for (def_id, signature) in &module.signatures.type_aliases {
            type_aliases.insert(
                GlobalDefId {
                    module_id: module.module_id,
                    def_id: *def_id,
                },
                ProgramTypeAliasSignature {
                    signature: signature.clone(),
                    interner: module.lowering.interner.clone(),
                },
            );
        }
    }
    type_aliases
}

pub(crate) fn collect_program_trait_impls(
    modules: &[ModuleSignatureInput<'_>],
) -> Vec<ProgramTraitImplSignature> {
    let mut trait_impls = Vec::new();
    for module in modules {
        for (local_index, impl_signature) in module.signatures.trait_impls.iter().enumerate() {
            let Some(trait_ty) = impl_signature.trait_ty else {
                continue;
            };
            let Some((trait_id, trait_args)) =
                trait_id_and_args(&module.lowering.interner, trait_ty)
            else {
                continue;
            };
            trait_impls.push(ProgramTraitImplSignature {
                module_id: module.module_id,
                local_index,
                generics: impl_signature.generics.clone(),
                target_ty: impl_signature.target_ty,
                trait_id,
                trait_args,
                where_predicates: impl_signature.where_predicates.clone(),
                associated_types: impl_signature.associated_types.clone(),
                interner: module.lowering.interner.clone(),
            });
        }
    }
    trait_impls
}

fn trait_id_and_args(
    interner: &TyInterner,
    ty: nia_ids::InternedTyId,
) -> Option<(TraitId, Vec<nia_ids::InternedTyId>)> {
    match interner.get(ty) {
        Some(TyKind::Nominal { def_id, args }) => Some((TraitId::Source(*def_id), args.clone())),
        Some(TyKind::BuiltinTrait { trait_id, args }) => {
            Some((TraitId::Builtin(*trait_id), args.clone()))
        }
        _ => None,
    }
}

pub(crate) fn collect_extension_methods(
    modules: &[ExtensionModuleInput<'_>],
) -> (ExtensionMethods, Vec<Diagnostic>) {
    let mut extensions = ExtensionMethods::default();
    let mut diagnostics = Vec::new();
    let defs_by_module = modules
        .iter()
        .map(|module| (module.module.id, module.defs))
        .collect::<HashMap<_, _>>();
    for module in modules {
        validate_supertraits(module, &defs_by_module, &mut diagnostics);
    }
    let trait_signatures = modules
        .iter()
        .flat_map(|module| {
            module
                .signatures
                .traits
                .iter()
                .map(move |(def_id, signature)| {
                    (
                        GlobalDefId {
                            module_id: module.module.id,
                            def_id: *def_id,
                        },
                        TraitSignatureRef {
                            signature,
                            interner: &module.lowering.interner,
                        },
                    )
                })
        })
        .collect::<HashMap<_, _>>();
    let trait_impls = collect_extension_trait_impls(modules);
    for module in modules {
        let mut impl_index = 0;
        for item in &module.module.module.items {
            let nia_ast::ItemKind::Extend(extend) = &item.kind else {
                continue;
            };
            let Some(target_ty) = lowered_type(module, &extend.target) else {
                diagnostics.push(Diagnostic::user_error_at(
                    "E0201",
                    extend.target.span,
                    "extend target must resolve to a nominal type",
                ));
                continue;
            };
            let target_ty = module.normalization.normalize(target_ty);
            if !is_extendable_target(&module.lowering.interner, target_ty) {
                diagnostics.push(Diagnostic::user_error_at(
                    "E0201",
                    extend.target.span,
                    "extend target must be an extendable value type",
                ));
                continue;
            }
            let trait_id = extend.trait_ref.as_ref().and_then(|trait_ref| {
                trait_ref_id(module, trait_ref, &defs_by_module, &mut diagnostics)
            });
            let trait_args = extend
                .trait_ref
                .as_ref()
                .and_then(|trait_ref| trait_ref_ty_args(module, trait_ref, trait_id))
                .unwrap_or_default();
            let where_predicates = where_predicates(module, &extend.where_clause);
            if trait_id.is_none() {
                for associated_type in &extend.associated_types {
                    diagnostics.push(Diagnostic::user_error_at(
                        "E0201",
                        associated_type.span,
                        "associated type definitions are only allowed in trait implementations",
                    ));
                }
            }
            match trait_id {
                Some(TraitId::Source(trait_id)) => {
                    validate_trait_impl(
                        module,
                        extend,
                        target_ty,
                        trait_id,
                        &trait_signatures,
                        &trait_impls,
                        &mut diagnostics,
                    );
                }
                Some(TraitId::Builtin(trait_id)) => {
                    validate_builtin_trait_impl(
                        module,
                        extend,
                        target_ty,
                        trait_id,
                        &trait_impls,
                        &mut diagnostics,
                    );
                }
                None => {}
            }
            for method in &extend.methods {
                let Some(method_id) = module.defs.def_nodes.get(&method.function.node_key) else {
                    continue;
                };
                let mut impl_generics = extend.generics.clone();
                if matches!(
                    module.lowering.interner.get(target_ty),
                    Some(TyKind::TraitObjectPointee { .. })
                ) && !impl_generics.iter().any(|generic| generic == "Self")
                {
                    impl_generics.push("Self".to_string());
                }
                extensions.insert(
                    module.module.id,
                    ExtensionMethod {
                        name: method.function.name.clone(),
                        def_id: GlobalDefId {
                            module_id: module.module.id,
                            def_id: method_id,
                        },
                        impl_index,
                        impl_generics,
                        target_ty,
                        trait_id,
                        trait_args: trait_args.clone(),
                        where_predicates: where_predicates.clone(),
                        visibility: method.vis,
                    },
                );
            }
            impl_index += 1;
        }
    }
    (extensions, diagnostics)
}

pub(crate) fn collect_extension_associated_values(
    modules: &[ExtensionModuleInput<'_>],
) -> (ExtensionAssociatedValues, Vec<Diagnostic>) {
    let mut values = ExtensionAssociatedValues::default();
    let mut diagnostics = Vec::new();
    for module in modules {
        let mut impl_index = 0;
        for item in &module.module.module.items {
            let nia_ast::ItemKind::Extend(extend) = &item.kind else {
                continue;
            };
            let Some(target_ty) = lowered_type(module, &extend.target) else {
                diagnostics.push(Diagnostic::user_error_at(
                    "E0201",
                    extend.target.span,
                    "extend target must resolve to a nominal type",
                ));
                continue;
            };
            let target_ty = module.normalization.normalize(target_ty);
            if !is_extendable_target(&module.lowering.interner, target_ty) {
                diagnostics.push(Diagnostic::user_error_at(
                    "E0201",
                    extend.target.span,
                    "extend target must be an extendable value type",
                ));
                continue;
            }
            for associated_value in &extend.associated_values {
                let binding = &associated_value.binding;
                let Some(def_id) = module.defs.def_nodes.get(&binding.node_key) else {
                    continue;
                };
                values.insert(
                    module.module.id,
                    ExtensionAssociatedValue {
                        name: binding.name.clone(),
                        def_id: GlobalDefId {
                            module_id: module.module.id,
                            def_id,
                        },
                        impl_index,
                        target_ty,
                        visibility: associated_value.vis,
                    },
                );
            }
            impl_index += 1;
        }
    }
    (values, diagnostics)
}

#[derive(Clone, Copy)]
struct TraitSignatureRef<'a> {
    signature: &'a TraitSignature,
    interner: &'a TyInterner,
}

fn validate_supertraits(
    module: &ExtensionModuleInput<'_>,
    defs_by_module: &HashMap<nia_ids::ModuleId, &DefCollection>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for item in &module.module.module.items {
        let nia_ast::ItemKind::Trait(item_trait) = &item.kind else {
            continue;
        };
        for supertrait in &item_trait.supertraits {
            let _ = trait_ref_id(module, supertrait, defs_by_module, diagnostics);
        }
    }
}

fn collect_extension_trait_impls(
    modules: &[ExtensionModuleInput<'_>],
) -> Vec<ProgramTraitImplSignature> {
    let signature_inputs = modules
        .iter()
        .map(|module| ModuleSignatureInput {
            module_id: module.module.id,
            defs: module.defs,
            lowering: module.lowering,
            signatures: module.signatures,
        })
        .collect::<Vec<_>>();
    collect_program_trait_impls(&signature_inputs)
}

fn trait_ref_id(
    module: &ExtensionModuleInput<'_>,
    trait_ref: &nia_ast::TypeRef,
    defs_by_module: &HashMap<nia_ids::ModuleId, &DefCollection>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<TraitId> {
    let Some(ty) = lowered_type(module, trait_ref) else {
        diagnostics.push(Diagnostic::user_error_at(
            "E0201",
            trait_ref.span,
            "trait implementation target must resolve to a trait",
        ));
        return None;
    };
    let ty = module.normalization.normalize(ty);
    match module.lowering.interner.get(ty).cloned() {
        Some(TyKind::Nominal { def_id, .. }) => {
            if !matches!(
                defs_by_module
                    .get(&def_id.module_id)
                    .and_then(|defs| defs.defs.get(def_id.def_id))
                    .map(|def| def.kind),
                Some(nia_defs::DefKind::Trait)
            ) {
                diagnostics.push(Diagnostic::user_error_at(
                    "E0201",
                    trait_ref.span,
                    "trait implementation target must be a trait",
                ));
                return None;
            }
            Some(TraitId::Source(def_id))
        }
        Some(TyKind::BuiltinTrait { trait_id, .. }) => Some(TraitId::Builtin(trait_id)),
        _ => {
            diagnostics.push(Diagnostic::user_error_at(
                "E0201",
                trait_ref.span,
                "trait implementation target must be a trait",
            ));
            None
        }
    }
}

fn validate_trait_impl(
    module: &ExtensionModuleInput<'_>,
    extend: &nia_ast::ExtendItem,
    target_ty: nia_ids::InternedTyId,
    trait_id: GlobalDefId,
    trait_signatures: &HashMap<GlobalDefId, TraitSignatureRef<'_>>,
    trait_impls: &[ProgramTraitImplSignature],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(trait_signature) = trait_signatures.get(&trait_id).copied() else {
        return;
    };
    for associated_type in &extend.associated_types {
        if !trait_signature
            .signature
            .associated_types
            .iter()
            .any(|required| required.name == associated_type.name)
        {
            diagnostics.push(Diagnostic::user_error_at(
                "E0201",
                associated_type.span,
                format!(
                    "associated type `{}` is not a member of implemented trait",
                    associated_type.name
                ),
            ));
        }
    }
    for required in &trait_signature.signature.associated_types {
        if !extend
            .associated_types
            .iter()
            .any(|associated_type| associated_type.name == required.name)
        {
            diagnostics.push(Diagnostic::user_error_at(
                "E0201",
                extend.target.span,
                format!("missing definition for associated type `{}`", required.name),
            ));
        }
    }
    for method in &extend.methods {
        if !trait_signature
            .signature
            .methods
            .iter()
            .any(|required| required.name == method.function.name)
        {
            diagnostics.push(Diagnostic::user_error_at(
                "E0201",
                method.function.span,
                format!(
                    "method `{}` is not a member of implemented trait",
                    method.function.name
                ),
            ));
        }
    }
    let trait_args = extend
        .trait_ref
        .as_ref()
        .and_then(|trait_ref| trait_ref_args(module, trait_ref, trait_id))
        .unwrap_or_default();
    validate_supertrait_impls(
        module,
        extend,
        target_ty,
        trait_signature,
        &trait_args,
        trait_impls,
        diagnostics,
    );
    let mut comparison_interner = module.normalization.interner.clone();
    for required in &trait_signature.signature.methods {
        let Some(method) = extend
            .methods
            .iter()
            .find(|method| method.function.name == required.name)
        else {
            if !required.has_default {
                diagnostics.push(Diagnostic::user_error_at(
                    "E0201",
                    extend.target.span,
                    format!(
                        "missing implementation for trait method `{}`",
                        required.name
                    ),
                ));
            }
            continue;
        };
        let Some(method_id) = module.defs.def_nodes.get(&method.function.node_key) else {
            continue;
        };
        let Some(actual) = module.signatures.functions.get(&method_id) else {
            continue;
        };
        let required_signature = import_trait_method_signature(TraitMethodImport {
            target_interner: &mut comparison_interner,
            module,
            source_interner: trait_signature.interner,
            signature: &required.signature,
            trait_generics: &trait_signature.signature.generics,
            trait_args: &trait_args,
            self_ty: target_ty,
            trait_id,
            extend,
        });
        let actual_signature = normalize_impl_method_signature(ImplMethodSignatureNormalize {
            target_interner: &mut comparison_interner,
            module,
            source_interner: &module.lowering.interner,
            signature: actual,
            trait_args: &trait_args,
            self_ty: target_ty,
            trait_id,
            extend,
        });
        if !trait_method_signature_matches(&required_signature, &actual_signature) {
            diagnostics.push(Diagnostic::user_error_at(
                "E0201",
                method.function.span,
                format!(
                    "implementation of trait method `{}` does not match the trait signature",
                    required.name
                ),
            ));
        }
    }
}

fn validate_builtin_trait_impl(
    module: &ExtensionModuleInput<'_>,
    extend: &nia_ast::ExtendItem,
    target_ty: nia_ids::InternedTyId,
    trait_id: BuiltinTrait,
    trait_impls: &[ProgramTraitImplSignature],
    diagnostics: &mut Vec<Diagnostic>,
) {
    if builtin_trait_impl_overlaps_intrinsic(module, target_ty, trait_id, extend) {
        diagnostics.push(Diagnostic::user_error_at(
            "E0201",
            extend.target.span,
            format!(
                "implementation of `{}` overlaps a compiler-proven implementation",
                trait_id.name()
            ),
        ));
        return;
    }
    for associated_type in &extend.associated_types {
        if !trait_id.has_associated_type(&associated_type.name) {
            diagnostics.push(Diagnostic::user_error_at(
                "E0201",
                associated_type.span,
                format!(
                    "associated type `{}` is not a member of implemented trait",
                    associated_type.name
                ),
            ));
        }
    }
    for associated_type_name in trait_id
        .associated_types()
        .iter()
        .map(|associated_type| associated_type.name())
    {
        if trait_id.has_associated_type(associated_type_name)
            && !extend
                .associated_types
                .iter()
                .any(|associated_type| associated_type.name == associated_type_name)
        {
            diagnostics.push(Diagnostic::user_error_at(
                "E0201",
                extend.target.span,
                format!("missing definition for associated type `{associated_type_name}`"),
            ));
        }
    }
    validate_builtin_supertrait_impls(
        module,
        extend,
        target_ty,
        trait_id,
        trait_impls,
        diagnostics,
    );
    let expected_methods = trait_id.required_methods();
    for method in &extend.methods {
        if !expected_methods
            .iter()
            .any(|expected_method| expected_method.name() == method.function.name)
        {
            diagnostics.push(Diagnostic::user_error_at(
                "E0201",
                method.function.span,
                format!(
                    "method `{}` is not a member of implemented trait",
                    method.function.name
                ),
            ));
        }
    }
    for expected_method in expected_methods {
        let matching_methods = extend
            .methods
            .iter()
            .filter(|method| method.function.name == expected_method.name())
            .collect::<Vec<_>>();
        match matching_methods.as_slice() {
            [] => diagnostics.push(Diagnostic::user_error_at(
                "E0201",
                extend.target.span,
                format!(
                    "missing implementation for trait method `{}`",
                    expected_method.name()
                ),
            )),
            [method] => {
                let Some(method_id) = module.defs.def_nodes.get(&method.function.node_key) else {
                    return;
                };
                let Some(actual) = module.signatures.functions.get(&method_id) else {
                    return;
                };
                if !builtin_trait_method_signature_matches(
                    module,
                    extend,
                    actual,
                    trait_id,
                    *expected_method,
                ) {
                    diagnostics.push(Diagnostic::user_error_at("E0201", 
                        method.function.span,
                        format!(
                            "implementation of trait method `{}` does not match the trait signature",
                            expected_method.name()
                        ),
                    ));
                }
            }
            _ => diagnostics.push(Diagnostic::user_error_at(
                "E0201",
                extend.target.span,
                format!(
                    "duplicate implementation for trait method `{}`",
                    expected_method.name()
                ),
            )),
        }
    }
}

fn validate_builtin_supertrait_impls(
    module: &ExtensionModuleInput<'_>,
    extend: &nia_ast::ExtendItem,
    target_ty: nia_ids::InternedTyId,
    trait_id: BuiltinTrait,
    trait_impls: &[ProgramTraitImplSignature],
    diagnostics: &mut Vec<Diagnostic>,
) {
    for supertrait in trait_id.supertraits() {
        let supertrait_args = if supertrait.preserves_trait_args {
            extend
                .trait_ref
                .as_ref()
                .and_then(|trait_ref| builtin_trait_ref_args(module, trait_ref, trait_id))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if !has_matching_trait_impl(
            &module.lowering.interner,
            target_ty,
            TraitId::Builtin(supertrait.trait_id),
            &supertrait_args,
            trait_impls,
        ) {
            diagnostics.push(Diagnostic::user_error_at(
                "E0201",
                extend.target.span,
                format!(
                    "implementation of trait requires explicit implementation of supertrait `{}`",
                    supertrait.trait_id.name()
                ),
            ));
        }
    }
}

fn builtin_trait_impl_overlaps_intrinsic(
    module: &ExtensionModuleInput<'_>,
    target_ty: nia_ids::InternedTyId,
    trait_id: BuiltinTrait,
    extend: &nia_ast::ExtendItem,
) -> bool {
    let target_ty = module.normalization.normalize(target_ty);
    let trait_args = extend
        .trait_ref
        .as_ref()
        .and_then(|trait_ref| builtin_trait_ref_args(module, trait_ref, trait_id))
        .unwrap_or_default();
    IntrinsicOverlap {
        interner: &module.lowering.interner,
        normalization: module.normalization,
        is_enum: |ty| match module
            .lowering
            .interner
            .get(module.normalization.normalize(ty))
        {
            Some(TyKind::Nominal { def_id, .. }) if def_id.module_id == module.module.id => {
                module.signatures.enums.contains_key(&def_id.def_id)
            }
            _ => false,
        },
    }
    .overlaps_builtin_trait(target_ty, trait_id, &trait_args)
}

fn builtin_trait_method_signature_matches(
    module: &ExtensionModuleInput<'_>,
    extend: &nia_ast::ExtendItem,
    actual: &FunctionSignature,
    trait_id: BuiltinTrait,
    method: BuiltinTraitMethod,
) -> bool {
    if actual.params.len() != method.param_count()
        || actual.return_type == module.lowering.interner.error()
    {
        return false;
    }
    match (trait_id, method) {
        (BuiltinTrait::DerefRead, BuiltinTraitMethod::DerefRead)
        | (BuiltinTrait::Deref, BuiltinTraitMethod::Deref)
        | (BuiltinTrait::IndexRead, BuiltinTraitMethod::IndexRead)
        | (BuiltinTrait::Index, BuiltinTraitMethod::Index) => {
            builtin_place_trait_method_signature_matches(module, extend, actual, trait_id, method)
        }
        (BuiltinTrait::SliceRead, BuiltinTraitMethod::SliceRead)
        | (BuiltinTrait::Slice, BuiltinTraitMethod::Slice) => {
            builtin_slice_trait_method_signature_matches(module, extend, actual, trait_id, method)
        }
        (BuiltinTrait::Iterator, BuiltinTraitMethod::IteratorNext) => {
            builtin_iterator_method_signature_matches(module, extend, actual)
        }
        (BuiltinTrait::Len, BuiltinTraitMethod::Len) => {
            builtin_len_method_signature_matches(module, actual)
        }
        (BuiltinTrait::Start, BuiltinTraitMethod::Start)
        | (BuiltinTrait::End, BuiltinTraitMethod::End) => {
            builtin_bound_method_signature_matches(module, extend, actual)
        }
        _ => true,
    }
}

fn builtin_place_trait_method_signature_matches(
    module: &ExtensionModuleInput<'_>,
    extend: &nia_ast::ExtendItem,
    actual: &FunctionSignature,
    trait_id: BuiltinTrait,
    method: BuiltinTraitMethod,
) -> bool {
    let Some(receiver) = actual.params.first().and_then(|param| param.receiver) else {
        return false;
    };
    let Some(expected_receiver) = method
        .place_receiver_kind()
        .map(receiver_kind_to_ast_receiver_kind)
    else {
        return false;
    };
    if receiver != expected_receiver {
        return false;
    }
    let Some(TyKind::Pointer { is_readonly, elem }) =
        module.lowering.interner.get(actual.return_type)
    else {
        return false;
    };
    let expected_const = matches!(trait_id, BuiltinTrait::DerefRead | BuiltinTrait::IndexRead);
    if *is_readonly != expected_const {
        return false;
    }
    let assoc_name = match trait_id {
        BuiltinTrait::DerefRead | BuiltinTrait::Deref => BuiltinTrait::TARGET_ASSOC_TYPE,
        BuiltinTrait::IndexRead | BuiltinTrait::Index => BuiltinTrait::OUTPUT_ASSOC_TYPE,
        _ => return false,
    };
    let Some(associated_type) = extend
        .associated_types
        .iter()
        .find(|associated_type| associated_type.name == assoc_name)
        .and_then(|associated_type| lowered_type(module, &associated_type.ty))
    else {
        return false;
    };
    types_equivalent(module.lowering, *elem, associated_type)
}

fn builtin_slice_trait_method_signature_matches(
    module: &ExtensionModuleInput<'_>,
    extend: &nia_ast::ExtendItem,
    actual: &FunctionSignature,
    trait_id: BuiltinTrait,
    method: BuiltinTraitMethod,
) -> bool {
    let Some(receiver) = actual.params.first().and_then(|param| param.receiver) else {
        return false;
    };
    let expected_receiver = receiver_kind_to_ast_receiver_kind(method.receiver_kind());
    if receiver != expected_receiver {
        return false;
    }
    let Some(range_param) = actual.params.get(1) else {
        return false;
    };
    let Some(range_ty) = extend
        .trait_ref
        .as_ref()
        .and_then(|trait_ref| builtin_trait_ref_args(module, trait_ref, trait_id))
        .and_then(|args| args.first().copied())
    else {
        return false;
    };
    if !types_equivalent(module.lowering, range_param.ty, range_ty) {
        return false;
    }
    let Some(output) = extend
        .associated_types
        .iter()
        .find(|associated_type| associated_type.name == BuiltinTrait::OUTPUT_ASSOC_TYPE)
        .and_then(|associated_type| lowered_type(module, &associated_type.ty))
    else {
        return false;
    };
    types_equivalent(module.lowering, actual.return_type, output)
}

fn builtin_iterator_method_signature_matches(
    module: &ExtensionModuleInput<'_>,
    extend: &nia_ast::ExtendItem,
    actual: &FunctionSignature,
) -> bool {
    if actual.params.first().and_then(|param| param.receiver) != Some(nia_ast::ReceiverKind::Ref) {
        return false;
    }
    let Some(item) = extend
        .associated_types
        .iter()
        .find(|associated_type| associated_type.name == BuiltinTrait::ITEM_ASSOC_TYPE)
        .and_then(|associated_type| lowered_type(module, &associated_type.ty))
    else {
        return false;
    };
    let actual_return = module.normalization.normalize(actual.return_type);
    let Some(TyKind::Optional { elem }) = module.lowering.interner.get(actual_return) else {
        return false;
    };
    types_equivalent(module.lowering, *elem, item)
}

fn builtin_len_method_signature_matches(
    module: &ExtensionModuleInput<'_>,
    actual: &FunctionSignature,
) -> bool {
    if actual.params.first().and_then(|param| param.receiver)
        != Some(nia_ast::ReceiverKind::RefReadOnly)
    {
        return false;
    }
    types_equivalent(
        module.lowering,
        actual.return_type,
        module.lowering.interner.primitive(PrimitiveTy::Usize),
    )
}

fn builtin_bound_method_signature_matches(
    module: &ExtensionModuleInput<'_>,
    extend: &nia_ast::ExtendItem,
    actual: &FunctionSignature,
) -> bool {
    if actual.params.first().and_then(|param| param.receiver)
        != Some(nia_ast::ReceiverKind::RefReadOnly)
    {
        return false;
    }
    let Some(output) = extend
        .associated_types
        .iter()
        .find(|associated_type| associated_type.name == BuiltinTrait::OUTPUT_ASSOC_TYPE)
        .and_then(|associated_type| lowered_type(module, &associated_type.ty))
    else {
        return false;
    };
    types_equivalent(module.lowering, actual.return_type, output)
}

fn receiver_kind_to_ast_receiver_kind(kind: BuiltinReceiverKind) -> nia_ast::ReceiverKind {
    match kind {
        BuiltinReceiverKind::RefReadOnly => nia_ast::ReceiverKind::RefReadOnly,
        BuiltinReceiverKind::Ref => nia_ast::ReceiverKind::Ref,
        BuiltinReceiverKind::Value => nia_ast::ReceiverKind::Value,
    }
}

fn builtin_trait_ref_args(
    module: &ExtensionModuleInput<'_>,
    trait_ref: &nia_ast::TypeRef,
    trait_id: BuiltinTrait,
) -> Option<Vec<nia_ids::InternedTyId>> {
    let ty = lowered_type(module, trait_ref)?;
    let ty = module.normalization.normalize(ty);
    match module.lowering.interner.get(ty) {
        Some(TyKind::BuiltinTrait {
            trait_id: found,
            args,
        }) if *found == trait_id => Some(args.clone()),
        _ => None,
    }
}

fn trait_ref_args(
    module: &ExtensionModuleInput<'_>,
    trait_ref: &nia_ast::TypeRef,
    trait_id: GlobalDefId,
) -> Option<Vec<nia_ids::InternedTyId>> {
    let ty = lowered_type(module, trait_ref)?;
    let ty = module.normalization.normalize(ty);
    match module.lowering.interner.get(ty) {
        Some(TyKind::Nominal { def_id, args }) if *def_id == trait_id => Some(args.clone()),
        _ => None,
    }
}

fn trait_ref_ty_args(
    module: &ExtensionModuleInput<'_>,
    trait_ref: &nia_ast::TypeRef,
    expected_trait_id: Option<TraitId>,
) -> Option<Vec<nia_ids::InternedTyId>> {
    let ty = lowered_type(module, trait_ref)?;
    let ty = module.normalization.normalize(ty);
    match (expected_trait_id, module.lowering.interner.get(ty)) {
        (Some(TraitId::Source(expected)), Some(TyKind::Nominal { def_id, args }))
            if *def_id == expected =>
        {
            Some(args.clone())
        }
        (
            Some(TraitId::Builtin(expected)),
            Some(TyKind::BuiltinTrait {
                trait_id: found,
                args,
            }),
        ) if *found == expected => Some(args.clone()),
        _ => None,
    }
}

fn validate_supertrait_impls(
    module: &ExtensionModuleInput<'_>,
    extend: &nia_ast::ExtendItem,
    target_ty: nia_ids::InternedTyId,
    trait_signature: TraitSignatureRef<'_>,
    trait_args: &[nia_ids::InternedTyId],
    trait_impls: &[ProgramTraitImplSignature],
    diagnostics: &mut Vec<Diagnostic>,
) {
    for supertrait in &trait_signature.signature.supertraits {
        let mut comparison_interner = module.lowering.interner.clone();
        let supertrait = import_trait_bound(
            &mut comparison_interner,
            module,
            trait_signature.interner,
            *supertrait,
            &trait_signature.signature.generics,
            trait_args,
        );
        let Some(TyKind::Nominal {
            def_id: supertrait_id,
            args: supertrait_args,
        }) = comparison_interner.get(supertrait).cloned()
        else {
            continue;
        };
        if !has_matching_trait_impl(
            &comparison_interner,
            target_ty,
            TraitId::Source(supertrait_id),
            &supertrait_args,
            trait_impls,
        ) {
            diagnostics.push(Diagnostic::user_error_at(
                "E0201",
                extend.target.span,
                format!(
                    "implementation of trait requires explicit implementation of supertrait `{}`",
                    trait_name(module, supertrait_id)
                ),
            ));
        }
    }
}

fn import_trait_bound(
    target_interner: &mut TyInterner,
    module: &ExtensionModuleInput<'_>,
    source_interner: &TyInterner,
    ty: nia_ids::InternedTyId,
    trait_generics: &[String],
    trait_args: &[nia_ids::InternedTyId],
) -> nia_ids::InternedTyId {
    let substitutions = trait_generics
        .iter()
        .zip(trait_args)
        .map(|(generic, arg)| (generic.clone(), *arg))
        .collect::<HashMap<_, _>>();
    substitute_imported_type(
        target_interner,
        module,
        source_interner,
        ty,
        &substitutions,
        None,
    )
}

fn has_matching_trait_impl(
    interner: &TyInterner,
    target_ty: nia_ids::InternedTyId,
    trait_id: TraitId,
    trait_args: &[nia_ids::InternedTyId],
    trait_impls: &[ProgramTraitImplSignature],
) -> bool {
    trait_impls.iter().any(|impl_signature| {
        if impl_signature.trait_id != trait_id {
            return false;
        }
        let mut comparison_interner = interner.clone();
        let impl_target_ty = import_type_into(
            &mut comparison_interner,
            &impl_signature.interner,
            impl_signature.target_ty,
        );
        let impl_trait_args = impl_signature
            .trait_args
            .iter()
            .map(|arg| import_type_into(&mut comparison_interner, &impl_signature.interner, *arg))
            .collect::<Vec<_>>();
        types_equivalent_in_interner(&comparison_interner, impl_target_ty, target_ty)
            && impl_trait_args.len() == trait_args.len()
            && impl_trait_args.iter().zip(trait_args).all(|(left, right)| {
                types_equivalent_in_interner(&comparison_interner, *left, *right)
            })
    })
}

fn types_equivalent(
    lowering: &TypeLowering,
    left: nia_ids::InternedTyId,
    right: nia_ids::InternedTyId,
) -> bool {
    types_equivalent_with_const_exprs(&lowering.interner, &lowering.const_exprs, left, right)
}

fn types_equivalent_in_interner(
    interner: &TyInterner,
    left: nia_ids::InternedTyId,
    right: nia_ids::InternedTyId,
) -> bool {
    types_equivalent_with_const_exprs(interner, &HashMap::new(), left, right)
}

fn types_equivalent_with_const_exprs(
    interner: &TyInterner,
    const_exprs: &HashMap<nia_ids::GlobalConstExprId, nia_ast::Expr>,
    left: nia_ids::InternedTyId,
    right: nia_ids::InternedTyId,
) -> bool {
    if left == right {
        return true;
    }
    SignatureTypeEquivalence {
        interner,
        const_exprs,
    }
    .compute_same_type_for_equiv(left, right)
}

struct SignatureTypeEquivalence<'a> {
    interner: &'a TyInterner,
    const_exprs: &'a HashMap<nia_ids::GlobalConstExprId, nia_ast::Expr>,
}

impl TypeEquivalence for SignatureTypeEquivalence<'_> {
    fn ty_kind_for_equiv(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.interner.get(ty)
    }

    fn same_array_len_for_equiv(&self, left: &ArrayLenTy, right: &ArrayLenTy) -> bool {
        if left == right {
            return true;
        }
        match (left, right) {
            (ArrayLenTy::Infer, ArrayLenTy::Infer) => true,
            (ArrayLenTy::ConstValue(left), ArrayLenTy::ConstValue(right)) => left == right,
            (
                ArrayLenTy::Builtin {
                    builtin: left_builtin,
                    ty: left_ty,
                },
                ArrayLenTy::Builtin {
                    builtin: right_builtin,
                    ty: right_ty,
                },
            ) => left_builtin == right_builtin && self.same_type_for_equiv(*left_ty, *right_ty),
            _ => self
                .literal_array_len_value(left)
                .zip(self.literal_array_len_value(right))
                .is_some_and(|(left, right)| left == right),
        }
    }

    fn same_type_for_equiv(&self, left: InternedTyId, right: InternedTyId) -> bool {
        types_equivalent_with_const_exprs(self.interner, self.const_exprs, left, right)
    }
}

impl SignatureTypeEquivalence<'_> {
    fn literal_array_len_value(&self, len: &ArrayLenTy) -> Option<u64> {
        match len {
            ArrayLenTy::ConstValue(value) => Some(*value),
            ArrayLenTy::ConstExpr(id) => self
                .const_exprs
                .get(id)
                .and_then(literal_array_len_expr_value),
            _ => None,
        }
    }
}

fn literal_array_len_expr_value(expr: &nia_ast::Expr) -> Option<u64> {
    let nia_ast::ExprKind::Integer(text) = &expr.kind else {
        return None;
    };
    let value = nia_comptime_engine::eval_int_literal(text).ok()?;
    u64::try_from(value).ok()
}

fn trait_name(module: &ExtensionModuleInput<'_>, trait_id: GlobalDefId) -> String {
    module
        .defs
        .defs
        .get(trait_id.def_id)
        .filter(|_| trait_id.module_id == module.module.id)
        .map(|def| def.name.clone())
        .unwrap_or_else(|| format!("trait#{}.{}", trait_id.module_id.0, trait_id.def_id.0))
}

fn import_trait_method_signature(import: TraitMethodImport<'_>) -> FunctionSignature {
    let mut substitutions = import
        .trait_generics
        .iter()
        .zip(import.trait_args)
        .map(|(generic, arg)| (generic.clone(), *arg))
        .collect::<HashMap<_, _>>();
    substitutions.insert("Self".to_string(), import.self_ty);
    let mut signature = import.signature.clone();
    signature.params = signature
        .params
        .iter()
        .map(|param| ParamSignature {
            name: param.name.clone(),
            receiver: param.receiver,
            ty: substitute_imported_type(
                import.target_interner,
                import.module,
                import.source_interner,
                param.ty,
                &substitutions,
                Some(ProjectionImplContext {
                    trait_id: import.trait_id,
                    trait_args: import.trait_args,
                    self_ty: import.self_ty,
                    extend: import.extend,
                }),
            ),
            span: param.span,
        })
        .collect();
    signature.return_type = substitute_imported_type(
        import.target_interner,
        import.module,
        import.source_interner,
        signature.return_type,
        &substitutions,
        Some(ProjectionImplContext {
            trait_id: import.trait_id,
            trait_args: import.trait_args,
            self_ty: import.self_ty,
            extend: import.extend,
        }),
    );
    signature
}

fn normalize_impl_method_signature(import: ImplMethodSignatureNormalize<'_>) -> FunctionSignature {
    let substitutions = HashMap::new();
    let context = Some(ProjectionImplContext {
        trait_id: import.trait_id,
        trait_args: import.trait_args,
        self_ty: import.self_ty,
        extend: import.extend,
    });
    let mut signature = import.signature.clone();
    signature.params = signature
        .params
        .iter()
        .map(|param| ParamSignature {
            name: param.name.clone(),
            receiver: param.receiver,
            ty: substitute_imported_type(
                import.target_interner,
                import.module,
                import.source_interner,
                param.ty,
                &substitutions,
                context,
            ),
            span: param.span,
        })
        .collect();
    signature.return_type = substitute_imported_type(
        import.target_interner,
        import.module,
        import.source_interner,
        signature.return_type,
        &substitutions,
        context,
    );
    signature
}

struct TraitMethodImport<'a> {
    target_interner: &'a mut TyInterner,
    module: &'a ExtensionModuleInput<'a>,
    source_interner: &'a TyInterner,
    signature: &'a FunctionSignature,
    // Required trait methods are authored in the trait module/interner but are
    // checked against an impl in the current module/interner. These fields keep
    // the substitution environment and projection-impl context in one place.
    trait_generics: &'a [String],
    trait_args: &'a [nia_ids::InternedTyId],
    self_ty: nia_ids::InternedTyId,
    trait_id: GlobalDefId,
    extend: &'a nia_ast::ExtendItem,
}

struct ImplMethodSignatureNormalize<'a> {
    target_interner: &'a mut TyInterner,
    module: &'a ExtensionModuleInput<'a>,
    source_interner: &'a TyInterner,
    signature: &'a FunctionSignature,
    trait_args: &'a [nia_ids::InternedTyId],
    self_ty: nia_ids::InternedTyId,
    trait_id: GlobalDefId,
    extend: &'a nia_ast::ExtendItem,
}

#[derive(Clone, Copy)]
struct ProjectionImplContext<'a> {
    trait_id: GlobalDefId,
    trait_args: &'a [nia_ids::InternedTyId],
    self_ty: nia_ids::InternedTyId,
    extend: &'a nia_ast::ExtendItem,
}

fn substitute_imported_type(
    target_interner: &mut TyInterner,
    module: &ExtensionModuleInput<'_>,
    source_interner: &TyInterner,
    ty: nia_ids::InternedTyId,
    substitutions: &HashMap<String, nia_ids::InternedTyId>,
    projection_context: Option<ProjectionImplContext<'_>>,
) -> nia_ids::InternedTyId {
    match source_interner.get(ty) {
        Some(TyKind::GenericParam(name)) => substitutions
            .get(name)
            .copied()
            .unwrap_or_else(|| import_type_into(target_interner, source_interner, ty)),
        Some(TyKind::Pointer { is_readonly, elem }) => {
            let is_readonly = *is_readonly;
            let elem = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *elem,
                substitutions,
                projection_context,
            );
            target_interner.intern(TyKind::Pointer { is_readonly, elem })
        }
        Some(TyKind::Slice { is_readonly, elem }) => {
            let is_readonly = *is_readonly;
            let elem = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *elem,
                substitutions,
                projection_context,
            );
            target_interner.intern(TyKind::Slice { is_readonly, elem })
        }
        Some(TyKind::SlicePointee { elem }) => {
            let elem = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *elem,
                substitutions,
                projection_context,
            );
            target_interner.intern(TyKind::SlicePointee { elem })
        }
        Some(TyKind::Array { len, elem }) => {
            let len = len.clone();
            let elem = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *elem,
                substitutions,
                projection_context,
            );
            target_interner.intern(TyKind::Array { len, elem })
        }
        Some(TyKind::Range { kind, bound }) => {
            let bound = bound.map(|bound| {
                substitute_imported_type(
                    target_interner,
                    module,
                    source_interner,
                    bound,
                    substitutions,
                    projection_context,
                )
            });
            target_interner.intern(TyKind::Range { kind: *kind, bound })
        }
        Some(TyKind::Optional { elem }) => {
            let elem = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *elem,
                substitutions,
                projection_context,
            );
            target_interner.intern(TyKind::Optional { elem })
        }
        Some(TyKind::ErrorUnion { error, value }) => {
            let error = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *error,
                substitutions,
                projection_context,
            );
            let value = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *value,
                substitutions,
                projection_context,
            );
            target_interner.intern(TyKind::ErrorUnion { error, value })
        }
        Some(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        }) => {
            let params = params
                .iter()
                .map(|param| {
                    substitute_imported_type(
                        target_interner,
                        module,
                        source_interner,
                        *param,
                        substitutions,
                        projection_context,
                    )
                })
                .collect();
            let return_type = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *return_type,
                substitutions,
                projection_context,
            );
            target_interner.intern(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic: *is_variadic,
            })
        }
        Some(TyKind::Nominal { def_id, args }) => {
            let args = args
                .iter()
                .map(|arg| {
                    substitute_imported_type(
                        target_interner,
                        module,
                        source_interner,
                        *arg,
                        substitutions,
                        projection_context,
                    )
                })
                .collect();
            target_interner.intern(TyKind::Nominal {
                def_id: *def_id,
                args,
            })
        }
        Some(TyKind::BuiltinTrait { trait_id, args }) => {
            let args = args
                .iter()
                .map(|arg| {
                    substitute_imported_type(
                        target_interner,
                        module,
                        source_interner,
                        *arg,
                        substitutions,
                        projection_context,
                    )
                })
                .collect();
            target_interner.intern(TyKind::BuiltinTrait {
                trait_id: *trait_id,
                args,
            })
        }
        Some(TyKind::TraitObject {
            is_readonly,
            trait_id,
            trait_args,
            associated_type_bindings,
        }) => {
            let trait_args = trait_args
                .iter()
                .map(|arg| {
                    substitute_imported_type(
                        target_interner,
                        module,
                        source_interner,
                        *arg,
                        substitutions,
                        projection_context,
                    )
                })
                .collect();
            let associated_type_bindings = associated_type_bindings
                .iter()
                .map(|binding| nia_ty::AssociatedTypeBindingTy {
                    trait_id: binding.trait_id,
                    trait_args: binding
                        .trait_args
                        .iter()
                        .map(|arg| {
                            substitute_imported_type(
                                target_interner,
                                module,
                                source_interner,
                                *arg,
                                substitutions,
                                projection_context,
                            )
                        })
                        .collect(),
                    name: binding.name.clone(),
                    ty: substitute_imported_type(
                        target_interner,
                        module,
                        source_interner,
                        binding.ty,
                        substitutions,
                        projection_context,
                    ),
                })
                .collect();
            target_interner.intern(TyKind::TraitObject {
                is_readonly: *is_readonly,
                trait_id: *trait_id,
                trait_args,
                associated_type_bindings,
            })
        }
        Some(TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            associated_type_bindings,
        }) => {
            let trait_args = trait_args
                .iter()
                .map(|arg| {
                    substitute_imported_type(
                        target_interner,
                        module,
                        source_interner,
                        *arg,
                        substitutions,
                        projection_context,
                    )
                })
                .collect();
            let associated_type_bindings = associated_type_bindings
                .iter()
                .map(|binding| nia_ty::AssociatedTypeBindingTy {
                    trait_id: binding.trait_id,
                    trait_args: binding
                        .trait_args
                        .iter()
                        .map(|arg| {
                            substitute_imported_type(
                                target_interner,
                                module,
                                source_interner,
                                *arg,
                                substitutions,
                                projection_context,
                            )
                        })
                        .collect(),
                    name: binding.name.clone(),
                    ty: substitute_imported_type(
                        target_interner,
                        module,
                        source_interner,
                        binding.ty,
                        substitutions,
                        projection_context,
                    ),
                })
                .collect();
            target_interner.intern(TyKind::TraitObjectPointee {
                trait_id: *trait_id,
                trait_args,
                associated_type_bindings,
            })
        }
        Some(TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            name,
        }) => {
            let self_ty = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *self_ty,
                substitutions,
                projection_context,
            );
            let trait_args = trait_args
                .iter()
                .map(|arg| {
                    substitute_imported_type(
                        target_interner,
                        module,
                        source_interner,
                        *arg,
                        substitutions,
                        projection_context,
                    )
                })
                .collect::<Vec<_>>();
            if let Some(context) = projection_context
                && *trait_id == TraitId::Source(context.trait_id)
                && self_ty == context.self_ty
                && trait_args == context.trait_args
                && let Some(associated_type) = context
                    .extend
                    .associated_types
                    .iter()
                    .find(|associated_type| associated_type.name == *name)
            {
                return module
                    .lowering
                    .node_type_uses
                    .get(&associated_type.ty.node_key)
                    .copied()
                    .unwrap_or_else(|| target_interner.error());
            }
            target_interner.intern(TyKind::Projection {
                self_ty,
                trait_id: *trait_id,
                trait_args,
                name: name.clone(),
            })
        }
        Some(
            TyKind::Error | TyKind::ComptimeOnly | TyKind::Primitive(_) | TyKind::Vector { .. },
        )
        | None => import_type_into(target_interner, source_interner, ty),
    }
}

fn trait_method_signature_matches(
    required: &nia_item_signatures::FunctionSignature,
    actual: &nia_item_signatures::FunctionSignature,
) -> bool {
    required.generics == actual.generics
        && required.where_predicates == actual.where_predicates
        && required.params.len() == actual.params.len()
        && required
            .params
            .iter()
            .zip(actual.params.iter())
            .all(|(required, actual)| {
                required.receiver == actual.receiver && required.ty == actual.ty
            })
        && required.return_type == actual.return_type
        && required.is_variadic == actual.is_variadic
}

fn is_extendable_target(interner: &TyInterner, ty: nia_ids::InternedTyId) -> bool {
    match interner.get(ty) {
        Some(TyKind::Error | TyKind::ComptimeOnly) | None => false,
        Some(TyKind::Primitive(PrimitiveTy::Never)) => false,
        Some(TyKind::Array { len, .. }) => !matches!(len, nia_ty::ArrayLenTy::Infer),
        Some(
            TyKind::Primitive(_)
            | TyKind::Vector { .. }
            | TyKind::Pointer { .. }
            | TyKind::Slice { .. }
            | TyKind::FunctionPointer { .. }
            | TyKind::Nominal { .. }
            | TyKind::BuiltinTrait { .. }
            | TyKind::SlicePointee { .. }
            | TyKind::TraitObject { .. }
            | TyKind::TraitObjectPointee { .. }
            | TyKind::Projection { .. }
            | TyKind::Range { .. }
            | TyKind::Optional { .. }
            | TyKind::ErrorUnion { .. }
            | TyKind::GenericParam(_),
        ) => true,
    }
}

pub(crate) fn visible_extensions_for_module(
    module_id: nia_ids::ModuleId,
    imports: &nia_imports::ImportAliasMap,
    public_surfaces: &PublicSurfaces,
    defs_by_module: &HashMap<nia_ids::ModuleId, DefCollection>,
    normalizations: &HashMap<nia_ids::ModuleId, TypeNormalization>,
    extensions: &ExtensionMethods,
    associated_values: &ExtensionAssociatedValues,
) -> VisibleExtensionsForModule {
    let imported_modules = transitive_import_closure(module_id, imports);
    let Some(current_normalization) = normalizations.get(&module_id) else {
        return VisibleExtensionsForModule {
            methods: VisibleExtensionMethods::default(),
            interner: TyInterner::default(),
        };
    };
    let mut target_interner = current_normalization.interner.clone();
    let mut visible = VisibleExtensionMethods::default();
    for method in extensions.visible_methods(module_id, imported_modules.iter().copied()) {
        let Some(method_defs) = defs_by_module.get(&method.def_id.module_id) else {
            continue;
        };
        if method_defs.defs.get(method.def_id.def_id).is_none() {
            continue;
        }
        let trait_is_visible = method.trait_id.is_some_and(|trait_id| {
            trait_id_is_visible(
                module_id,
                &imported_modules,
                trait_id,
                public_surfaces,
                defs_by_module,
            )
        });
        let Some(method_normalization) = normalizations.get(&method.def_id.module_id) else {
            continue;
        };
        let target_ty = method_normalization.normalize(method.target_ty);
        let target_ty = import_type_into(
            &mut target_interner,
            &method_normalization.interner,
            target_ty,
        );
        visible.insert(
            method.impl_index,
            target_ty,
            VisibleExtensionMethod {
                name: method.name.clone(),
                def_id: method.def_id,
                impl_index: method.impl_index,
                impl_generics: method.impl_generics.clone(),
                trait_id: method.trait_id,
                trait_args: method
                    .trait_args
                    .iter()
                    .map(|arg| {
                        let arg = method_normalization.normalize(*arg);
                        import_type_into(&mut target_interner, &method_normalization.interner, arg)
                    })
                    .collect(),
                where_predicates: import_where_predicates(
                    &mut target_interner,
                    &method_normalization.interner,
                    &method.where_predicates,
                ),
                is_callable: method.def_id.module_id == module_id
                    || method.visibility == nia_ast::Visibility::Public,
                is_trait_witness: trait_is_visible,
            },
        );
    }
    for value in associated_values.visible_values(module_id, imported_modules.iter().copied()) {
        let Some(value_defs) = defs_by_module.get(&value.def_id.module_id) else {
            continue;
        };
        if value_defs.defs.get(value.def_id.def_id).is_none() {
            continue;
        }
        let Some(value_normalization) = normalizations.get(&value.def_id.module_id) else {
            continue;
        };
        let target_ty = value_normalization.normalize(value.target_ty);
        let target_ty = import_type_into(
            &mut target_interner,
            &value_normalization.interner,
            target_ty,
        );
        visible.insert_associated_value(
            value.impl_index,
            target_ty,
            VisibleExtensionAssociatedValue {
                name: value.name.clone(),
                def_id: value.def_id,
            },
        );
    }
    VisibleExtensionsForModule {
        methods: visible,
        interner: target_interner,
    }
}

fn trait_id_is_visible(
    current_module: nia_ids::ModuleId,
    imported_modules: &[nia_ids::ModuleId],
    trait_id: TraitId,
    public_surfaces: &PublicSurfaces,
    defs_by_module: &HashMap<nia_ids::ModuleId, DefCollection>,
) -> bool {
    let TraitId::Source(trait_id) = trait_id else {
        return true;
    };
    if trait_id.module_id == current_module {
        return true;
    }
    if imported_modules.contains(&trait_id.module_id) {
        return defs_by_module
            .get(&trait_id.module_id)
            .and_then(|defs| defs.defs.get(trait_id.def_id))
            .is_some_and(|def| def.visibility == nia_ast::Visibility::Public);
    }
    public_surfaces.get(current_module).is_some_and(|surface| {
        surface.types.values().any(|item| {
            item.target_module == trait_id.module_id
                && item.target_def_id == trait_id.def_id
                && item.namespace == PublicNamespace::Type
        })
    })
}

fn import_where_predicates(
    target_interner: &mut TyInterner,
    source_interner: &TyInterner,
    predicates: &[WherePredicateSignature],
) -> Vec<WherePredicateSignature> {
    predicates
        .iter()
        .map(|predicate| WherePredicateSignature {
            ty: import_type_into(target_interner, source_interner, predicate.ty),
            bounds: predicate
                .bounds
                .iter()
                .map(|bound| WhereBoundSignature {
                    trait_ty: import_type_into(target_interner, source_interner, bound.trait_ty),
                    associated_type_bindings: bound
                        .associated_type_bindings
                        .iter()
                        .map(|binding| AssociatedTypeBindingSignature {
                            name: binding.name.clone(),
                            ty: import_type_into(target_interner, source_interner, binding.ty),
                            span: binding.span,
                        })
                        .collect(),
                    span: bound.span,
                })
                .collect(),
            span: predicate.span,
        })
        .collect()
}

fn transitive_import_closure(
    module_id: nia_ids::ModuleId,
    imports: &nia_imports::ImportAliasMap,
) -> Vec<nia_ids::ModuleId> {
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    if let Some(aliases) = imports.module_aliases(module_id) {
        for alias in aliases.values() {
            queue.push_back(alias.target);
        }
    }

    while let Some(imported) = queue.pop_front() {
        if imported == module_id || !seen.insert(imported) {
            continue;
        }
        if let Some(aliases) = imports.module_aliases(imported) {
            for alias in aliases.values() {
                queue.push_back(alias.target);
            }
        }
    }

    let mut modules = seen.into_iter().collect::<Vec<_>>();
    modules.sort();
    modules
}
