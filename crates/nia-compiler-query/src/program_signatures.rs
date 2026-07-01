// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet, VecDeque};

use nia_defs::{
    AssociatedTypeBindingSignature, DefCollection, ExtensionAssociatedValue,
    ExtensionAssociatedValues, ExtensionMethod, ExtensionMethods, PublicNamespace, PublicSurfaces,
    VisibleExtensionAssociatedValue, VisibleExtensionMethod, VisibleExtensionMethods,
    WhereBoundSignature, WherePredicateSignature,
};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{
    BuiltinTrait, BuiltinTraitMethod, GlobalDefId, InternedTyId, ReceiverKind, TraitImplId,
    Visibility,
};
use nia_item_signatures::{
    FunctionSignature, ItemSignatures, ParamSignature, ProgramComptimeSignature,
    ProgramEnumSignature, ProgramFunctionSignature, ProgramGlobalSignature, ProgramStructSignature,
    ProgramTraitImplSignature, ProgramTraitSignature, ProgramTypeAliasSignature,
    ProgramUnionSignature, TraitImplSignature, TraitSignature,
};
use nia_trait_solve::{
    AssociatedTypeProjectionEq, IntrinsicOverlap, TraitGoal, TraitSolverContext,
};
use nia_ty::{
    ArrayLenTy, ConstExprSummary, PrimitiveTy, TraitId, TyInterner, TyKind, TypeEquivalence,
    import_type_into,
};
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;

type TypeNormalizationResolver<'a> = &'a dyn Fn(nia_ids::ModuleId) -> Option<TypeNormalization>;

pub(crate) struct ModuleSignatureInput<'a> {
    pub(crate) module_id: nia_ids::ModuleId,
    pub(crate) defs: &'a DefCollection,
    pub(crate) lowering: &'a TypeLowering,
    pub(crate) signatures: &'a ItemSignatures,
}

pub(crate) struct ExtensionModuleInput<'a> {
    pub(crate) module_id: nia_ids::ModuleId,
    pub(crate) defs: &'a DefCollection,
    pub(crate) lowering: &'a TypeLowering,
    pub(crate) signatures: &'a ItemSignatures,
    pub(crate) function_signatures: &'a ItemSignatures,
    pub(crate) type_signatures: &'a ItemSignatures,
    pub(crate) normalization: &'a TypeNormalization,
}

pub(crate) struct ExtensionMethodIndexModuleInput<'a> {
    pub(crate) module_id: nia_ids::ModuleId,
    pub(crate) defs: &'a DefCollection,
    pub(crate) lowering: &'a TypeLowering,
    pub(crate) signatures: &'a ItemSignatures,
    pub(crate) normalization: &'a TypeNormalization,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VisibleExtensionsForModule {
    pub(crate) methods: VisibleExtensionMethods,
    pub(crate) interner: TyInterner,
}

pub(crate) fn collect_program_functions_excluding(
    modules: &[ModuleSignatureInput<'_>],
    excluded: &HashSet<GlobalDefId>,
) -> HashMap<GlobalDefId, ProgramFunctionSignature> {
    let mut functions = HashMap::new();
    for module in modules {
        for (def_id, signature) in &module.signatures.functions {
            let global_def_id = GlobalDefId {
                module_id: module.module_id,
                def_id: *def_id,
            };
            if excluded.contains(&global_def_id) {
                continue;
            }
            functions.insert(
                global_def_id,
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
        for impl_signature in &module.signatures.trait_impls {
            if impl_signature.builtin.is_some() {
                continue;
            }
            let Some(trait_ty) = impl_signature.trait_ty else {
                continue;
            };
            let Some((trait_id, trait_args, trait_const_args)) =
                trait_id_and_args(&module.lowering.interner, trait_ty)
            else {
                continue;
            };
            trait_impls.push(ProgramTraitImplSignature {
                module_id: module.module_id,
                impl_id: impl_signature.impl_id,
                builtin: impl_signature.builtin.clone(),
                generics: impl_signature.generics.clone(),
                target_ty: impl_signature.target_ty,
                trait_id,
                trait_args,
                trait_const_args,
                where_predicates: impl_signature.where_predicates.clone(),
                associated_types: impl_signature.associated_types.clone(),
                interner: module.lowering.interner.clone(),
            });
        }
    }
    trait_impls
}

pub(crate) fn collect_valid_program_trait_impls(
    modules: &[ExtensionModuleInput<'_>],
) -> Vec<ProgramTraitImplSignature> {
    collect_program_trait_impls(
        &modules
            .iter()
            .map(|module| ModuleSignatureInput {
                module_id: module.module_id,
                defs: module.defs,
                lowering: module.lowering,
                signatures: module.signatures,
            })
            .collect::<Vec<_>>(),
    )
    .into_iter()
    .filter(|impl_signature| {
        let Some(module) = modules
            .iter()
            .find(|module| module.module_id == impl_signature.module_id)
        else {
            return false;
        };
        let Some(module_impl_signature) =
            trait_impl_signature_by_id(module.signatures, impl_signature.impl_id)
        else {
            return false;
        };
        !matches!(impl_signature.trait_id, TraitId::Builtin(trait_id)
        if builtin_trait_impl_overlaps_intrinsic(
            module,
            impl_signature.target_ty,
            trait_id,
            module_impl_signature,
        ))
    })
    .collect()
}

pub(crate) fn collect_invalid_trait_impl_method_ids(
    modules: &[ExtensionModuleInput<'_>],
) -> HashSet<GlobalDefId> {
    let mut invalid_methods = HashSet::new();
    for module in modules {
        for impl_signature in &module.signatures.trait_impls {
            let target_ty = module.normalization.normalize(impl_signature.target_ty);
            let trait_id = impl_signature.trait_ty.and_then(|trait_ty| {
                trait_id_and_args(&module.lowering.interner, trait_ty)
                    .map(|(trait_id, _, _)| trait_id)
            });
            let is_invalid = matches!(trait_id, Some(TraitId::Builtin(trait_id))
                if builtin_trait_impl_overlaps_intrinsic(module, target_ty, trait_id, impl_signature));
            if !is_invalid {
                continue;
            }
            invalid_methods.extend(impl_signature.methods.iter().map(|method| GlobalDefId {
                module_id: module.module_id,
                def_id: method.def_id,
            }));
        }
    }
    invalid_methods
}

fn trait_id_and_args(
    interner: &TyInterner,
    ty: nia_ids::InternedTyId,
) -> Option<(
    TraitId,
    Vec<nia_ids::InternedTyId>,
    Vec<nia_ty::ConstGenericArg>,
)> {
    match interner.get(ty) {
        Some(TyKind::Nominal {
            def_id,
            args,
            const_args,
        }) => Some((TraitId::Source(*def_id), args.clone(), const_args.clone())),
        Some(TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            trait_const_args,
            ..
        })
        | Some(TyKind::TraitObject {
            trait_id,
            trait_args,
            trait_const_args,
            ..
        }) => Some((*trait_id, trait_args.clone(), trait_const_args.clone())),
        Some(TyKind::BuiltinTrait { trait_id, args }) => {
            Some((TraitId::Builtin(*trait_id), args.clone(), Vec::new()))
        }
        _ => None,
    }
}

fn trait_impl_signature_by_id(
    signatures: &ItemSignatures,
    impl_id: TraitImplId,
) -> Option<&TraitImplSignature> {
    signatures
        .trait_impls
        .iter()
        .find(|signature| signature.impl_id == impl_id)
}

pub(crate) fn collect_extension_methods(
    modules: &[ExtensionModuleInput<'_>],
    trait_impls: &[ProgramTraitImplSignature],
) -> (ExtensionMethods, Vec<Diagnostic>) {
    let mut extensions = ExtensionMethods::default();
    let mut diagnostics = Vec::new();
    let defs_by_module = modules
        .iter()
        .map(|module| (module.module_id, module.defs))
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
                            module_id: module.module_id,
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
    for module in modules {
        for impl_signature in &module.signatures.trait_impls {
            let target_ty = module.normalization.normalize(impl_signature.target_ty);
            if !is_extendable_target(&module.lowering.interner, target_ty) {
                diagnostics.push(Diagnostic::user_error_at(
                    codes::NAME_RESOLUTION,
                    impl_signature.span,
                    "extend target must be an extendable value type",
                ));
                continue;
            }
            let trait_id = impl_trait_id(module, impl_signature, &defs_by_module, &mut diagnostics);
            let trait_args = impl_trait_args(module, impl_signature, trait_id).unwrap_or_default();
            let where_predicates =
                normalize_where_predicates(&module.normalization, &impl_signature.where_predicates);
            if trait_id.is_none() {
                for associated_type in &impl_signature.associated_types {
                    diagnostics.push(Diagnostic::user_error_at(
                        codes::NAME_RESOLUTION,
                        associated_type.span,
                        "associated type definitions are only allowed in trait implementations",
                    ));
                }
            }
            let valid_trait_impl = match trait_id {
                Some(TraitId::Source(trait_id)) => validate_trait_impl(
                    module,
                    impl_signature,
                    target_ty,
                    trait_id,
                    &trait_signatures,
                    trait_impls,
                    &mut diagnostics,
                ),
                Some(TraitId::Builtin(trait_id)) => validate_builtin_trait_impl(
                    module,
                    impl_signature,
                    target_ty,
                    trait_id,
                    trait_impls,
                    &mut diagnostics,
                ),
                None => true,
            };
            if !valid_trait_impl {
                continue;
            }
            for method in &impl_signature.methods {
                let effective_generics =
                    extension_method_effective_generics(module, impl_signature, method, target_ty);
                extensions.insert_with_nominal_target(
                    module.module_id,
                    ExtensionMethod {
                        name: method.name.clone(),
                        def_id: GlobalDefId {
                            module_id: module.module_id,
                            def_id: method.def_id,
                        },
                        impl_id: impl_signature.impl_id,
                        effective_generics,
                        target_ty,
                        trait_id,
                        trait_args: trait_args.clone(),
                        where_predicates: where_predicates.clone(),
                        visibility: method.visibility,
                    },
                    nominal_target_def_id(&module.normalization.interner, target_ty),
                );
            }
        }
    }
    (extensions, diagnostics)
}

pub(crate) fn collect_extension_method_index(
    modules: &[ExtensionMethodIndexModuleInput<'_>],
) -> ExtensionMethods {
    let mut extensions = ExtensionMethods::default();
    let defs_by_module = modules
        .iter()
        .map(|module| (module.module_id, module.defs))
        .collect::<HashMap<_, _>>();
    for module in modules {
        for impl_signature in &module.signatures.trait_impls {
            let target_ty = module.normalization.normalize(impl_signature.target_ty);
            if !is_extendable_target(&module.lowering.interner, target_ty) {
                continue;
            }
            let trait_id = impl_trait_id_for_index(module, impl_signature, &defs_by_module);
            let trait_args =
                impl_trait_args_for_index(module, impl_signature, trait_id).unwrap_or_default();
            let where_predicates =
                normalize_where_predicates(&module.normalization, &impl_signature.where_predicates);
            for method in &impl_signature.methods {
                let effective_generics =
                    extension_method_effective_generics(module, impl_signature, method, target_ty);
                extensions.insert_with_nominal_target(
                    module.module_id,
                    ExtensionMethod {
                        name: method.name.clone(),
                        def_id: GlobalDefId {
                            module_id: module.module_id,
                            def_id: method.def_id,
                        },
                        impl_id: impl_signature.impl_id,
                        effective_generics,
                        target_ty,
                        trait_id,
                        trait_args: trait_args.clone(),
                        where_predicates: where_predicates.clone(),
                        visibility: method.visibility,
                    },
                    nominal_target_def_id(&module.normalization.interner, target_ty),
                );
            }
        }
    }
    extensions
}

fn extension_method_effective_generics(
    module: &impl ExtensionMethodModule,
    impl_signature: &TraitImplSignature,
    method: &nia_item_signatures::TraitImplMethodSignature,
    target_ty: InternedTyId,
) -> Vec<String> {
    let mut generics = impl_signature.generics.clone();
    if matches!(
        module.interner().get(target_ty),
        Some(TyKind::TraitObjectPointee { .. })
    ) && !generics.iter().any(|generic| generic == "Self")
    {
        generics.push("Self".to_string());
    }
    if let Some(def) = module.defs().defs.get(method.def_id) {
        generics.extend(def.generics.iter().cloned());
    }
    generics
}

trait ExtensionMethodModule {
    fn defs(&self) -> &DefCollection;
    fn interner(&self) -> &TyInterner;
}

impl ExtensionMethodModule for ExtensionModuleInput<'_> {
    fn defs(&self) -> &DefCollection {
        self.defs
    }

    fn interner(&self) -> &TyInterner {
        &self.lowering.interner
    }
}

impl ExtensionMethodModule for ExtensionMethodIndexModuleInput<'_> {
    fn defs(&self) -> &DefCollection {
        self.defs
    }

    fn interner(&self) -> &TyInterner {
        &self.lowering.interner
    }
}

fn normalize_where_predicates(
    normalization: &TypeNormalization,
    predicates: &[WherePredicateSignature],
) -> Vec<WherePredicateSignature> {
    predicates
        .iter()
        .map(|predicate| WherePredicateSignature {
            ty: normalization.normalize(predicate.ty),
            bounds: predicate
                .bounds
                .iter()
                .map(|bound| WhereBoundSignature {
                    trait_ty: normalization.normalize(bound.trait_ty),
                    associated_type_bindings: bound
                        .associated_type_bindings
                        .iter()
                        .map(|binding| AssociatedTypeBindingSignature {
                            name: binding.name.clone(),
                            ty: normalization.normalize(binding.ty),
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

pub(crate) fn collect_extension_associated_value_index(
    modules: &[ExtensionMethodIndexModuleInput<'_>],
) -> (ExtensionAssociatedValues, Vec<Diagnostic>) {
    let mut values = ExtensionAssociatedValues::default();
    let mut diagnostics = Vec::new();
    for module in modules {
        for impl_signature in &module.signatures.trait_impls {
            let target_ty = module.normalization.normalize(impl_signature.target_ty);
            if !is_extendable_target(&module.lowering.interner, target_ty) {
                diagnostics.push(Diagnostic::user_error_at(
                    codes::NAME_RESOLUTION,
                    impl_signature.span,
                    "extend target must be an extendable value type",
                ));
                continue;
            }
            for associated_value in &impl_signature.associated_values {
                values.insert_with_nominal_target(
                    module.module_id,
                    ExtensionAssociatedValue {
                        name: associated_value.name.clone(),
                        def_id: GlobalDefId {
                            module_id: module.module_id,
                            def_id: associated_value.def_id,
                        },
                        impl_id: impl_signature.impl_id,
                        target_ty,
                        visibility: associated_value.visibility,
                    },
                    nominal_target_def_id(&module.normalization.interner, target_ty),
                );
            }
        }
    }
    (values, diagnostics)
}

fn impl_trait_id(
    module: &ExtensionModuleInput<'_>,
    impl_signature: &TraitImplSignature,
    defs_by_module: &HashMap<nia_ids::ModuleId, &DefCollection>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<TraitId> {
    let Some(trait_ty) = impl_signature.trait_ty else {
        return None;
    };
    let span = impl_signature.trait_span.unwrap_or(impl_signature.span);
    let ty = module.normalization.normalize(trait_ty);
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
                    codes::NAME_RESOLUTION,
                    span,
                    "trait implementation target must be a trait",
                ));
                return None;
            }
            Some(TraitId::Source(def_id))
        }
        Some(TyKind::BuiltinTrait { trait_id, .. }) => Some(TraitId::Builtin(trait_id)),
        _ => {
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                span,
                "trait implementation target must be a trait",
            ));
            None
        }
    }
}

fn impl_trait_id_for_index(
    module: &ExtensionMethodIndexModuleInput<'_>,
    impl_signature: &TraitImplSignature,
    defs_by_module: &HashMap<nia_ids::ModuleId, &DefCollection>,
) -> Option<TraitId> {
    let trait_ty = impl_signature.trait_ty?;
    let ty = module.normalization.normalize(trait_ty);
    match module.lowering.interner.get(ty).cloned() {
        Some(TyKind::Nominal { def_id, .. }) => matches!(
            defs_by_module
                .get(&def_id.module_id)
                .and_then(|defs| defs.defs.get(def_id.def_id))
                .map(|def| def.kind),
            Some(nia_defs::DefKind::Trait)
        )
        .then_some(TraitId::Source(def_id)),
        Some(TyKind::BuiltinTrait { trait_id, .. }) => Some(TraitId::Builtin(trait_id)),
        _ => None,
    }
}

fn impl_trait_args(
    module: &ExtensionModuleInput<'_>,
    impl_signature: &TraitImplSignature,
    expected_trait_id: Option<TraitId>,
) -> Option<Vec<nia_ids::InternedTyId>> {
    let ty = module.normalization.normalize(impl_signature.trait_ty?);
    match (expected_trait_id, module.normalization.interner.get(ty)) {
        (Some(TraitId::Source(expected)), Some(TyKind::Nominal { def_id, args, .. }))
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

fn impl_trait_args_and_consts(
    module: &ExtensionModuleInput<'_>,
    impl_signature: &TraitImplSignature,
    expected_trait_id: Option<TraitId>,
) -> Option<(Vec<nia_ids::InternedTyId>, Vec<nia_ty::ConstGenericArg>)> {
    let ty = module.normalization.normalize(impl_signature.trait_ty?);
    match (expected_trait_id, module.normalization.interner.get(ty)) {
        (
            Some(TraitId::Source(expected)),
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }),
        ) if *def_id == expected => Some((args.clone(), const_args.clone())),
        (
            Some(TraitId::Builtin(expected)),
            Some(TyKind::BuiltinTrait {
                trait_id: found,
                args,
            }),
        ) if *found == expected => Some((args.clone(), Vec::new())),
        _ => None,
    }
}

fn impl_trait_args_for_index(
    module: &ExtensionMethodIndexModuleInput<'_>,
    impl_signature: &TraitImplSignature,
    expected_trait_id: Option<TraitId>,
) -> Option<Vec<nia_ids::InternedTyId>> {
    let ty = module.normalization.normalize(impl_signature.trait_ty?);
    match (expected_trait_id, module.normalization.interner.get(ty)) {
        (Some(TraitId::Source(expected)), Some(TyKind::Nominal { def_id, args, .. }))
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

fn associated_type_ty(
    impl_signature: &TraitImplSignature,
    name: &str,
) -> Option<nia_ids::InternedTyId> {
    impl_signature
        .associated_types
        .iter()
        .find(|associated_type| associated_type.name == name)
        .map(|associated_type| associated_type.ty)
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
    for trait_signature in module.signatures.traits.values() {
        for supertrait in &trait_signature.supertraits {
            let _ = supertrait_id(
                module,
                supertrait.ty,
                supertrait.span,
                defs_by_module,
                diagnostics,
            );
        }
    }
}

fn supertrait_id(
    module: &ExtensionModuleInput<'_>,
    ty: InternedTyId,
    span: nia_span::Span,
    defs_by_module: &HashMap<nia_ids::ModuleId, &DefCollection>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<TraitId> {
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
                    codes::NAME_RESOLUTION,
                    span,
                    "trait implementation target must be a trait",
                ));
                return None;
            }
            Some(TraitId::Source(def_id))
        }
        Some(TyKind::BuiltinTrait { trait_id, .. }) => Some(TraitId::Builtin(trait_id)),
        _ => {
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                span,
                "trait implementation target must be a trait",
            ));
            None
        }
    }
}

fn validate_trait_impl(
    module: &ExtensionModuleInput<'_>,
    impl_signature: &TraitImplSignature,
    target_ty: nia_ids::InternedTyId,
    trait_id: GlobalDefId,
    trait_signatures: &HashMap<GlobalDefId, TraitSignatureRef<'_>>,
    trait_impls: &[ProgramTraitImplSignature],
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let Some(trait_signature) = trait_signatures.get(&trait_id).copied() else {
        return false;
    };
    let start_len = diagnostics.len();
    for associated_type in &impl_signature.associated_types {
        if !trait_signature
            .signature
            .associated_types
            .iter()
            .any(|required| required.name == associated_type.name)
        {
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                associated_type.span,
                format!(
                    "associated type `{}` is not a member of implemented trait",
                    associated_type.name
                ),
            ));
        }
    }
    for required in &trait_signature.signature.associated_types {
        if !impl_signature
            .associated_types
            .iter()
            .any(|associated_type| associated_type.name == required.name)
        {
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                impl_signature.span,
                format!("missing definition for associated type `{}`", required.name),
            ));
        }
    }
    for method in &impl_signature.methods {
        if !trait_signature
            .signature
            .methods
            .iter()
            .any(|required| required.name == method.name)
        {
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                method.span,
                format!(
                    "method `{}` is not a member of implemented trait",
                    method.name
                ),
            ));
        }
    }
    let (trait_args, trait_const_args) =
        impl_trait_args_and_consts(module, impl_signature, Some(TraitId::Source(trait_id)))
            .unwrap_or_default();
    validate_supertrait_impls(
        module,
        impl_signature,
        target_ty,
        trait_signature,
        &trait_args,
        trait_impls,
        diagnostics,
    );
    let mut comparison_interner = module.normalization.interner.clone();
    for required in &trait_signature.signature.methods {
        let Some(method) = impl_signature
            .methods
            .iter()
            .find(|method| method.name == required.name)
        else {
            if !required.has_default {
                diagnostics.push(Diagnostic::user_error_at(
                    codes::NAME_RESOLUTION,
                    impl_signature.span,
                    format!(
                        "missing implementation for trait method `{}`",
                        required.name
                    ),
                ));
            }
            continue;
        };
        let Some(actual) = module.function_signatures.functions.get(&method.def_id) else {
            continue;
        };
        let required_signature = lower_trait_method_signature(TraitMethodImport {
            target_interner: &mut comparison_interner,
            module,
            source_interner: trait_signature.interner,
            signature: &required.signature,
            trait_generics: &trait_signature.signature.generics,
            trait_args: &trait_args,
            trait_const_args: &trait_const_args,
            self_ty: target_ty,
            trait_id,
            impl_signature,
        });
        let actual_signature = normalize_impl_method_signature(ImplMethodSignatureNormalize {
            target_interner: &mut comparison_interner,
            module,
            source_interner: &module.lowering.interner,
            signature: actual,
            trait_args: &trait_args,
            trait_const_args: &trait_const_args,
            self_ty: target_ty,
            trait_id,
            impl_signature,
        });
        if !trait_method_signature_matches(
            module,
            trait_impls,
            &mut comparison_interner,
            target_ty,
            TraitId::Source(trait_id),
            &trait_args,
            &trait_const_args,
            impl_signature,
            &trait_signatures,
            &required_signature,
            &actual_signature,
        ) {
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                method.span,
                format!(
                    "implementation of trait method `{}` does not match the trait signature",
                    required.name
                ),
            ));
        }
    }
    diagnostics.len() == start_len
}

fn validate_builtin_trait_impl(
    module: &ExtensionModuleInput<'_>,
    impl_signature: &TraitImplSignature,
    target_ty: nia_ids::InternedTyId,
    trait_id: BuiltinTrait,
    trait_impls: &[ProgramTraitImplSignature],
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let start_len = diagnostics.len();
    if builtin_trait_impl_overlaps_intrinsic(module, target_ty, trait_id, impl_signature) {
        diagnostics.push(Diagnostic::user_error_at(
            codes::NAME_RESOLUTION,
            impl_signature.span,
            format!(
                "implementation of `{}` overlaps a compiler-proven implementation",
                trait_id.name()
            ),
        ));
        return false;
    }
    for associated_type in &impl_signature.associated_types {
        if !trait_id.has_associated_type(&associated_type.name) {
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
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
            && !impl_signature
                .associated_types
                .iter()
                .any(|associated_type| associated_type.name == associated_type_name)
        {
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                impl_signature.span,
                format!("missing definition for associated type `{associated_type_name}`"),
            ));
        }
    }
    validate_builtin_supertrait_impls(
        module,
        impl_signature,
        target_ty,
        trait_id,
        trait_impls,
        diagnostics,
    );
    let expected_methods = trait_id.required_methods();
    for method in &impl_signature.methods {
        if !expected_methods
            .iter()
            .any(|expected_method| expected_method.name() == method.name)
        {
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                method.span,
                format!(
                    "method `{}` is not a member of implemented trait",
                    method.name
                ),
            ));
        }
    }
    for expected_method in expected_methods {
        let matching_methods = impl_signature
            .methods
            .iter()
            .filter(|method| method.name == expected_method.name())
            .collect::<Vec<_>>();
        match matching_methods.as_slice() {
            [] => diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                impl_signature.span,
                format!(
                    "missing implementation for trait method `{}`",
                    expected_method.name()
                ),
            )),
            [method] => {
                let Some(actual) = module.function_signatures.functions.get(&method.def_id) else {
                    return false;
                };
                if !builtin_trait_method_signature_matches(
                    module,
                    impl_signature,
                    actual,
                    trait_id,
                    *expected_method,
                ) {
                    diagnostics.push(Diagnostic::user_error_at(codes::NAME_RESOLUTION,
                        method.span,
                        format!(
                            "implementation of trait method `{}` does not match the trait signature",
                            expected_method.name()
                        ),
                    ));
                }
            }
            _ => diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                impl_signature.span,
                format!(
                    "duplicate implementation for trait method `{}`",
                    expected_method.name()
                ),
            )),
        }
    }
    diagnostics.len() == start_len
}

fn validate_builtin_supertrait_impls(
    module: &ExtensionModuleInput<'_>,
    impl_signature: &TraitImplSignature,
    target_ty: nia_ids::InternedTyId,
    trait_id: BuiltinTrait,
    trait_impls: &[ProgramTraitImplSignature],
    diagnostics: &mut Vec<Diagnostic>,
) {
    for supertrait in trait_id.supertraits() {
        let supertrait_args = if supertrait.preserves_trait_args {
            builtin_impl_trait_args(module, impl_signature, trait_id).unwrap_or_default()
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
                codes::NAME_RESOLUTION,
                impl_signature.span,
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
    impl_signature: &TraitImplSignature,
) -> bool {
    let target_ty = module.normalization.normalize(target_ty);
    let trait_args = builtin_impl_trait_args(module, impl_signature, trait_id).unwrap_or_default();
    IntrinsicOverlap {
        interner: &module.lowering.interner,
        normalization: module.normalization,
        is_enum: |ty| match module
            .lowering
            .interner
            .get(module.normalization.normalize(ty))
        {
            Some(TyKind::Nominal { def_id, .. }) if def_id.module_id == module.module_id => {
                module.type_signatures.enums.contains_key(&def_id.def_id)
            }
            _ => false,
        },
    }
    .overlaps_builtin_trait(target_ty, trait_id, &trait_args)
}

fn builtin_trait_method_signature_matches(
    module: &ExtensionModuleInput<'_>,
    impl_signature: &TraitImplSignature,
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
        (BuiltinTrait::Deref, BuiltinTraitMethod::Deref)
        | (BuiltinTrait::DerefMut, BuiltinTraitMethod::DerefMut)
        | (BuiltinTrait::Index, BuiltinTraitMethod::Index)
        | (BuiltinTrait::IndexMut, BuiltinTraitMethod::IndexMut) => {
            builtin_place_trait_method_signature_matches(
                module,
                impl_signature,
                actual,
                trait_id,
                method,
            )
        }
        (BuiltinTrait::Slice, BuiltinTraitMethod::Slice)
        | (BuiltinTrait::SliceMut, BuiltinTraitMethod::SliceMut) => {
            builtin_slice_trait_method_signature_matches(
                module,
                impl_signature,
                actual,
                trait_id,
                method,
            )
        }
        (BuiltinTrait::Iterator, BuiltinTraitMethod::IteratorNext) => {
            builtin_iterator_method_signature_matches(module, impl_signature, actual)
        }
        (BuiltinTrait::Len, BuiltinTraitMethod::Len) => {
            builtin_len_method_signature_matches(module, actual)
        }
        (BuiltinTrait::Start, BuiltinTraitMethod::Start)
        | (BuiltinTrait::End, BuiltinTraitMethod::End) => {
            builtin_bound_method_signature_matches(module, impl_signature, actual)
        }
        (BuiltinTrait::Char, BuiltinTraitMethod::Char) => {
            builtin_char_method_signature_matches(module, actual)
        }
        (BuiltinTrait::Iterable, BuiltinTraitMethod::IterableIter) => {
            builtin_iterable_method_signature_matches(module, impl_signature, actual)
        }
        _ => true,
    }
}

fn builtin_place_trait_method_signature_matches(
    module: &ExtensionModuleInput<'_>,
    impl_signature: &TraitImplSignature,
    actual: &FunctionSignature,
    trait_id: BuiltinTrait,
    method: BuiltinTraitMethod,
) -> bool {
    let Some(receiver) = actual.params.first().and_then(|param| param.receiver) else {
        return false;
    };
    let Some(expected_receiver) = method.place_receiver_kind() else {
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
    let expected_const = matches!(trait_id, BuiltinTrait::Deref | BuiltinTrait::Index);
    if *is_readonly != expected_const {
        return false;
    }
    let assoc_name = match trait_id {
        BuiltinTrait::Deref | BuiltinTrait::DerefMut => BuiltinTrait::TARGET_ASSOC_TYPE,
        BuiltinTrait::Index | BuiltinTrait::IndexMut => BuiltinTrait::OUTPUT_ASSOC_TYPE,
        _ => return false,
    };
    let Some(associated_type) = associated_type_ty(impl_signature, assoc_name) else {
        return false;
    };
    types_equivalent(module.lowering, *elem, associated_type)
}

fn builtin_slice_trait_method_signature_matches(
    module: &ExtensionModuleInput<'_>,
    impl_signature: &TraitImplSignature,
    actual: &FunctionSignature,
    trait_id: BuiltinTrait,
    method: BuiltinTraitMethod,
) -> bool {
    let Some(receiver) = actual.params.first().and_then(|param| param.receiver) else {
        return false;
    };
    if receiver != method.receiver_kind() {
        return false;
    }
    let Some(range_param) = actual.params.get(1) else {
        return false;
    };
    let Some(range_ty) = builtin_impl_trait_args(module, impl_signature, trait_id)
        .and_then(|args| args.first().copied())
    else {
        return false;
    };
    if !types_equivalent(module.lowering, range_param.ty, range_ty) {
        return false;
    }
    let Some(output) = associated_type_ty(impl_signature, BuiltinTrait::OUTPUT_ASSOC_TYPE) else {
        return false;
    };
    types_equivalent(module.lowering, actual.return_type, output)
}

fn builtin_iterator_method_signature_matches(
    module: &ExtensionModuleInput<'_>,
    impl_signature: &TraitImplSignature,
    actual: &FunctionSignature,
) -> bool {
    if actual.params.first().and_then(|param| param.receiver) != Some(ReceiverKind::Ref) {
        return false;
    }
    let Some(item) = associated_type_ty(impl_signature, BuiltinTrait::ITEM_ASSOC_TYPE) else {
        return false;
    };
    let actual_return = module.normalization.normalize(actual.return_type);
    let Some(TyKind::Optional { elem }) = module.lowering.interner.get(actual_return) else {
        return false;
    };
    types_equivalent(module.lowering, *elem, item)
}

fn builtin_iterable_method_signature_matches(
    module: &ExtensionModuleInput<'_>,
    impl_signature: &TraitImplSignature,
    actual: &FunctionSignature,
) -> bool {
    if actual.params.first().and_then(|param| param.receiver) != Some(ReceiverKind::RefReadOnly) {
        return false;
    }
    let Some(iter) = associated_type_ty(impl_signature, BuiltinTrait::ITER_ASSOC_TYPE) else {
        return false;
    };
    types_equivalent(module.lowering, actual.return_type, iter)
}

fn builtin_len_method_signature_matches(
    module: &ExtensionModuleInput<'_>,
    actual: &FunctionSignature,
) -> bool {
    if actual.params.first().and_then(|param| param.receiver) != Some(ReceiverKind::RefReadOnly) {
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
    impl_signature: &TraitImplSignature,
    actual: &FunctionSignature,
) -> bool {
    if actual.params.first().and_then(|param| param.receiver) != Some(ReceiverKind::RefReadOnly) {
        return false;
    }
    let Some(output) = associated_type_ty(impl_signature, BuiltinTrait::OUTPUT_ASSOC_TYPE) else {
        return false;
    };
    types_equivalent(module.lowering, actual.return_type, output)
}

fn builtin_char_method_signature_matches(
    module: &ExtensionModuleInput<'_>,
    actual: &FunctionSignature,
) -> bool {
    if actual.params.first().and_then(|param| param.receiver) != Some(ReceiverKind::Value) {
        return false;
    }
    let actual_return = module.normalization.normalize(actual.return_type);
    let Some(TyKind::Optional { elem }) = module.lowering.interner.get(actual_return) else {
        return false;
    };
    types_equivalent(
        module.lowering,
        *elem,
        module.lowering.interner.primitive(PrimitiveTy::Char),
    )
}

fn builtin_impl_trait_args(
    module: &ExtensionModuleInput<'_>,
    impl_signature: &TraitImplSignature,
    trait_id: BuiltinTrait,
) -> Option<Vec<nia_ids::InternedTyId>> {
    let ty = impl_signature.trait_ty?;
    let ty = module.normalization.normalize(ty);
    match module.lowering.interner.get(ty) {
        Some(TyKind::BuiltinTrait {
            trait_id: found,
            args,
        }) if *found == trait_id => Some(args.clone()),
        _ => None,
    }
}

fn validate_supertrait_impls(
    module: &ExtensionModuleInput<'_>,
    impl_signature: &TraitImplSignature,
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
            supertrait.ty,
            &trait_signature.signature.generics,
            trait_args,
        );
        let Some(TyKind::Nominal {
            def_id: supertrait_id,
            args: supertrait_args,
            ..
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
                codes::NAME_RESOLUTION,
                impl_signature.span,
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
        &HashMap::new(),
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
    types_equivalent_with_const_exprs(
        &lowering.interner,
        &lowering.const_expr_summaries,
        left,
        right,
    )
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
    const_exprs: &HashMap<nia_ids::GlobalConstExprId, ConstExprSummary>,
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
    const_exprs: &'a HashMap<nia_ids::GlobalConstExprId, ConstExprSummary>,
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
                .and_then(|summary| summary.literal_array_len),
            _ => None,
        }
    }
}

fn trait_name(module: &ExtensionModuleInput<'_>, trait_id: GlobalDefId) -> String {
    module
        .defs
        .defs
        .get(trait_id.def_id)
        .filter(|_| trait_id.module_id == module.module_id)
        .map(|def| def.name.clone())
        .unwrap_or_else(|| format!("trait#{}.{}", trait_id.module_id.0, trait_id.def_id.0))
}

fn lower_trait_method_signature(input: TraitMethodImport<'_>) -> FunctionSignature {
    let mut substitutions = input
        .trait_generics
        .iter()
        .zip(input.trait_args)
        .map(|(generic, arg)| (generic.clone(), *arg))
        .collect::<HashMap<_, _>>();
    substitutions.insert("Self".to_string(), input.self_ty);
    let const_substitutions = const_substitutions_from_self_describing_args(input.trait_const_args);
    let mut signature = input.signature.clone();
    signature.params = signature
        .params
        .iter()
        .map(|param| ParamSignature {
            name: param.name.clone(),
            receiver: param.receiver,
            ty: substitute_imported_type(
                input.target_interner,
                input.module,
                input.source_interner,
                param.ty,
                &substitutions,
                &const_substitutions,
                Some(ProjectionImplContext {
                    trait_id: input.trait_id,
                    trait_args: input.trait_args,
                    trait_const_args: input.trait_const_args,
                    self_ty: input.self_ty,
                    associated_types: &input.impl_signature.associated_types,
                }),
            ),
            span: param.span,
        })
        .collect();
    signature.return_type = substitute_imported_type(
        input.target_interner,
        input.module,
        input.source_interner,
        signature.return_type,
        &substitutions,
        &const_substitutions,
        Some(ProjectionImplContext {
            trait_id: input.trait_id,
            trait_args: input.trait_args,
            trait_const_args: input.trait_const_args,
            self_ty: input.self_ty,
            associated_types: &input.impl_signature.associated_types,
        }),
    );
    signature
}

fn normalize_impl_method_signature(input: ImplMethodSignatureNormalize<'_>) -> FunctionSignature {
    let substitutions = HashMap::new();
    let const_substitutions = HashMap::new();
    let context = Some(ProjectionImplContext {
        trait_id: input.trait_id,
        trait_args: input.trait_args,
        trait_const_args: input.trait_const_args,
        self_ty: input.self_ty,
        associated_types: &input.impl_signature.associated_types,
    });
    let mut signature = input.signature.clone();
    signature.params = signature
        .params
        .iter()
        .map(|param| ParamSignature {
            name: param.name.clone(),
            receiver: param.receiver,
            ty: substitute_imported_type(
                input.target_interner,
                input.module,
                input.source_interner,
                param.ty,
                &substitutions,
                &const_substitutions,
                context,
            ),
            span: param.span,
        })
        .collect();
    signature.return_type = substitute_imported_type(
        input.target_interner,
        input.module,
        input.source_interner,
        signature.return_type,
        &substitutions,
        &const_substitutions,
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
    trait_const_args: &'a [nia_ty::ConstGenericArg],
    self_ty: nia_ids::InternedTyId,
    trait_id: GlobalDefId,
    impl_signature: &'a TraitImplSignature,
}

struct ImplMethodSignatureNormalize<'a> {
    target_interner: &'a mut TyInterner,
    module: &'a ExtensionModuleInput<'a>,
    source_interner: &'a TyInterner,
    signature: &'a FunctionSignature,
    trait_args: &'a [nia_ids::InternedTyId],
    trait_const_args: &'a [nia_ty::ConstGenericArg],
    self_ty: nia_ids::InternedTyId,
    trait_id: GlobalDefId,
    impl_signature: &'a TraitImplSignature,
}

#[derive(Clone, Copy)]
struct ProjectionImplContext<'a> {
    trait_id: GlobalDefId,
    trait_args: &'a [nia_ids::InternedTyId],
    trait_const_args: &'a [nia_ty::ConstGenericArg],
    self_ty: nia_ids::InternedTyId,
    associated_types: &'a [nia_item_signatures::TraitImplAssociatedTypeSignature],
}

fn substitute_imported_type(
    target_interner: &mut TyInterner,
    module: &ExtensionModuleInput<'_>,
    source_interner: &TyInterner,
    ty: nia_ids::InternedTyId,
    substitutions: &HashMap<String, nia_ids::InternedTyId>,
    const_substitutions: &HashMap<String, nia_ty::ConstGenericArg>,
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
                const_substitutions,
                projection_context,
            );
            target_interner.intern(TyKind::Pointer { is_readonly, elem })
        }
        Some(TyKind::VolatilePointer { is_readonly, elem }) => {
            let is_readonly = *is_readonly;
            let elem = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *elem,
                substitutions,
                const_substitutions,
                projection_context,
            );
            target_interner.intern(TyKind::VolatilePointer { is_readonly, elem })
        }
        Some(TyKind::Slice { is_readonly, elem }) => {
            let is_readonly = *is_readonly;
            let elem = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *elem,
                substitutions,
                const_substitutions,
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
                const_substitutions,
                projection_context,
            );
            target_interner.intern(TyKind::SlicePointee { elem })
        }
        Some(TyKind::Array { len, elem }) => {
            let len = substitute_imported_array_len(len.clone(), const_substitutions);
            let elem = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *elem,
                substitutions,
                const_substitutions,
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
                    const_substitutions,
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
                const_substitutions,
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
                const_substitutions,
                projection_context,
            );
            let value = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *value,
                substitutions,
                const_substitutions,
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
                        const_substitutions,
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
                const_substitutions,
                projection_context,
            );
            target_interner.intern(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic: *is_variadic,
            })
        }
        Some(TyKind::Nominal {
            def_id,
            args,
            const_args,
        }) => {
            let args = args
                .iter()
                .map(|arg| {
                    substitute_imported_type(
                        target_interner,
                        module,
                        source_interner,
                        *arg,
                        substitutions,
                        const_substitutions,
                        projection_context,
                    )
                })
                .collect();
            let const_args = const_args
                .iter()
                .map(|arg| {
                    substitute_imported_const_arg(
                        target_interner,
                        module,
                        source_interner,
                        arg,
                        substitutions,
                        const_substitutions,
                        projection_context,
                    )
                })
                .collect();
            target_interner.intern(TyKind::Nominal {
                def_id: *def_id,
                args,
                const_args,
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
                        const_substitutions,
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
            trait_const_args,
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
                        const_substitutions,
                        projection_context,
                    )
                })
                .collect();
            let trait_const_args = trait_const_args
                .iter()
                .map(|arg| {
                    substitute_imported_const_arg(
                        target_interner,
                        module,
                        source_interner,
                        arg,
                        substitutions,
                        const_substitutions,
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
                                const_substitutions,
                                projection_context,
                            )
                        })
                        .collect(),
                    trait_const_args: binding
                        .trait_const_args
                        .iter()
                        .map(|arg| {
                            substitute_imported_const_arg(
                                target_interner,
                                module,
                                source_interner,
                                arg,
                                substitutions,
                                const_substitutions,
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
                        const_substitutions,
                        projection_context,
                    ),
                })
                .collect();
            target_interner.intern(TyKind::TraitObject {
                is_readonly: *is_readonly,
                trait_id: *trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            })
        }
        Some(TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            trait_const_args,
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
                        const_substitutions,
                        projection_context,
                    )
                })
                .collect();
            let trait_const_args = trait_const_args
                .iter()
                .map(|arg| {
                    substitute_imported_const_arg(
                        target_interner,
                        module,
                        source_interner,
                        arg,
                        substitutions,
                        const_substitutions,
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
                                const_substitutions,
                                projection_context,
                            )
                        })
                        .collect(),
                    trait_const_args: binding
                        .trait_const_args
                        .iter()
                        .map(|arg| {
                            substitute_imported_const_arg(
                                target_interner,
                                module,
                                source_interner,
                                arg,
                                substitutions,
                                const_substitutions,
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
                        const_substitutions,
                        projection_context,
                    ),
                })
                .collect();
            target_interner.intern(TyKind::TraitObjectPointee {
                trait_id: *trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            })
        }
        Some(TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            name,
        }) => {
            let self_ty = substitute_imported_type(
                target_interner,
                module,
                source_interner,
                *self_ty,
                substitutions,
                const_substitutions,
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
                        const_substitutions,
                        projection_context,
                    )
                })
                .collect::<Vec<_>>();
            let trait_const_args = trait_const_args
                .iter()
                .map(|arg| {
                    substitute_imported_const_arg(
                        target_interner,
                        module,
                        source_interner,
                        arg,
                        substitutions,
                        const_substitutions,
                        projection_context,
                    )
                })
                .collect::<Vec<_>>();
            if let Some(context) = projection_context
                && *trait_id == TraitId::Source(context.trait_id)
                && self_ty == context.self_ty
                && trait_args == context.trait_args
                && trait_const_args == context.trait_const_args
                && let Some(associated_type) = context
                    .associated_types
                    .iter()
                    .find(|associated_type| associated_type.name == *name)
            {
                let ty = module.normalization.normalize(associated_type.ty);
                return import_type_into(target_interner, &module.normalization.interner, ty);
            }
            target_interner.intern(TyKind::Projection {
                self_ty,
                trait_id: *trait_id,
                trait_args,
                trait_const_args,
                name: name.clone(),
            })
        }
        Some(
            TyKind::Error | TyKind::ComptimeOnly | TyKind::Primitive(_) | TyKind::Vector { .. },
        )
        | None => import_type_into(target_interner, source_interner, ty),
    }
}

fn const_substitutions_from_self_describing_args(
    const_args: &[nia_ty::ConstGenericArg],
) -> HashMap<String, nia_ty::ConstGenericArg> {
    const_args
        .iter()
        .filter_map(|arg| match &arg.value {
            nia_ty::ConstGenericValue::GenericParam(name) => Some((name.clone(), arg.clone())),
            _ => None,
        })
        .collect()
}

fn substitute_imported_const_arg(
    target_interner: &mut TyInterner,
    module: &ExtensionModuleInput<'_>,
    source_interner: &TyInterner,
    arg: &nia_ty::ConstGenericArg,
    substitutions: &HashMap<String, nia_ids::InternedTyId>,
    const_substitutions: &HashMap<String, nia_ty::ConstGenericArg>,
    projection_context: Option<ProjectionImplContext<'_>>,
) -> nia_ty::ConstGenericArg {
    if let nia_ty::ConstGenericValue::GenericParam(name) = &arg.value
        && let Some(substituted) = const_substitutions.get(name)
    {
        return substituted.clone();
    }
    nia_ty::ConstGenericArg {
        ty: substitute_imported_type(
            target_interner,
            module,
            source_interner,
            arg.ty,
            substitutions,
            const_substitutions,
            projection_context,
        ),
        value: arg.value.clone(),
    }
}

fn substitute_imported_array_len(
    len: nia_ty::ArrayLenTy,
    const_substitutions: &HashMap<String, nia_ty::ConstGenericArg>,
) -> nia_ty::ArrayLenTy {
    match len {
        nia_ty::ArrayLenTy::GenericParam(name) => const_substitutions
            .get(&name)
            .and_then(array_len_from_const_arg)
            .unwrap_or(nia_ty::ArrayLenTy::GenericParam(name)),
        len => len,
    }
}

fn array_len_from_const_arg(arg: &nia_ty::ConstGenericArg) -> Option<nia_ty::ArrayLenTy> {
    match &arg.value {
        nia_ty::ConstGenericValue::Int(value) => value
            .bits()
            .try_into()
            .ok()
            .map(nia_ty::ArrayLenTy::ConstValue),
        nia_ty::ConstGenericValue::GenericParam(name) => {
            Some(nia_ty::ArrayLenTy::GenericParam(name.clone()))
        }
        nia_ty::ConstGenericValue::ConstExpr(id) => Some(nia_ty::ArrayLenTy::ConstExpr(*id)),
        nia_ty::ConstGenericValue::Bool(_) | nia_ty::ConstGenericValue::Char(_) => None,
    }
}

fn trait_method_signature_matches(
    module: &ExtensionModuleInput<'_>,
    trait_impls: &[ProgramTraitImplSignature],
    interner: &mut TyInterner,
    self_ty: nia_ids::InternedTyId,
    trait_id: TraitId,
    trait_args: &[nia_ids::InternedTyId],
    trait_const_args: &[nia_ty::ConstGenericArg],
    impl_signature: &TraitImplSignature,
    trait_signatures: &HashMap<GlobalDefId, TraitSignatureRef<'_>>,
    required: &nia_item_signatures::FunctionSignature,
    actual: &nia_item_signatures::FunctionSignature,
) -> bool {
    let mut assumptions = vec![TraitGoal {
        self_ty,
        trait_id,
        trait_args: trait_args.to_vec(),
        trait_const_args: trait_const_args.to_vec(),
    }];
    let mut associated_type_assumptions = impl_signature
        .associated_types
        .iter()
        .map(|associated_type| AssociatedTypeProjectionEq {
            goal: TraitGoal {
                self_ty,
                trait_id,
                trait_args: trait_args.to_vec(),
                trait_const_args: trait_const_args.to_vec(),
            },
            name: associated_type.name.clone(),
            ty: import_type_into(
                interner,
                &module.normalization.interner,
                module.normalization.normalize(associated_type.ty),
            ),
        })
        .collect::<Vec<_>>();
    push_where_predicate_solver_assumptions(
        module,
        interner,
        &impl_signature.where_predicates,
        trait_signatures,
        &mut assumptions,
        &mut associated_type_assumptions,
    );
    let context = TraitSolverContext {
        normalization: module.normalization,
        trait_impls,
        layouts: None,
        local_module_id: module.module_id,
        local_enums: &module.signatures.enums,
        program_enums: None,
        const_expr_value: None,
        impl_is_visible: None,
    };
    let mut solver = context.solver_with_associated_type_assumptions(
        interner,
        &assumptions,
        &associated_type_assumptions,
    );
    required.generics == actual.generics
        && required.where_predicates == actual.where_predicates
        && required.params.len() == actual.params.len()
        && required
            .params
            .iter()
            .zip(actual.params.iter())
            .all(|(required, actual)| {
                required.receiver == actual.receiver
                    && solver.types_equivalent(required.ty, actual.ty)
            })
        && solver.types_equivalent(required.return_type, actual.return_type)
        && required.is_variadic == actual.is_variadic
}

fn push_where_predicate_solver_assumptions(
    module: &ExtensionModuleInput<'_>,
    interner: &mut TyInterner,
    predicates: &[WherePredicateSignature],
    trait_signatures: &HashMap<GlobalDefId, TraitSignatureRef<'_>>,
    assumptions: &mut Vec<TraitGoal>,
    associated_type_assumptions: &mut Vec<AssociatedTypeProjectionEq>,
) {
    for predicate in predicates {
        let self_ty = import_type_into(
            interner,
            &module.normalization.interner,
            module.normalization.normalize(predicate.ty),
        );
        for bound in &predicate.bounds {
            let trait_ty = import_type_into(
                interner,
                &module.normalization.interner,
                module.normalization.normalize(bound.trait_ty),
            );
            let Some((trait_id, trait_args, trait_const_args)) =
                trait_id_and_args(interner, trait_ty)
            else {
                continue;
            };
            push_trait_goal_assumption_with_supertraits(
                module,
                interner,
                trait_signatures,
                self_ty,
                trait_id,
                trait_args.clone(),
                trait_const_args.clone(),
                assumptions,
            );
            for binding in &bound.associated_type_bindings {
                let ty = import_type_into(
                    interner,
                    &module.normalization.interner,
                    module.normalization.normalize(binding.ty),
                );
                associated_type_assumptions.push(AssociatedTypeProjectionEq {
                    goal: TraitGoal {
                        self_ty,
                        trait_id,
                        trait_args: trait_args.clone(),
                        trait_const_args: trait_const_args.clone(),
                    },
                    name: binding.name.clone(),
                    ty,
                });
            }
        }
    }
}

fn push_trait_goal_assumption_with_supertraits(
    module: &ExtensionModuleInput<'_>,
    interner: &mut TyInterner,
    trait_signatures: &HashMap<GlobalDefId, TraitSignatureRef<'_>>,
    self_ty: InternedTyId,
    trait_id: TraitId,
    trait_args: Vec<InternedTyId>,
    trait_const_args: Vec<nia_ty::ConstGenericArg>,
    assumptions: &mut Vec<TraitGoal>,
) {
    push_trait_goal_assumption_with_supertraits_inner(
        module,
        interner,
        trait_signatures,
        self_ty,
        trait_id,
        trait_args,
        trait_const_args,
        assumptions,
        &mut HashSet::new(),
    );
}

fn push_trait_goal_assumption_with_supertraits_inner(
    module: &ExtensionModuleInput<'_>,
    interner: &mut TyInterner,
    trait_signatures: &HashMap<GlobalDefId, TraitSignatureRef<'_>>,
    self_ty: InternedTyId,
    trait_id: TraitId,
    trait_args: Vec<InternedTyId>,
    trait_const_args: Vec<nia_ty::ConstGenericArg>,
    assumptions: &mut Vec<TraitGoal>,
    visited: &mut HashSet<(TraitId, Vec<InternedTyId>, Vec<nia_ty::ConstGenericArg>)>,
) {
    if !visited.insert((trait_id, trait_args.clone(), trait_const_args.clone())) {
        return;
    }
    if !assumptions.iter().any(|assumption| {
        assumption.self_ty == self_ty
            && assumption.trait_id == trait_id
            && assumption.trait_args == trait_args
            && assumption.trait_const_args == trait_const_args
    }) {
        assumptions.push(TraitGoal {
            self_ty,
            trait_id,
            trait_args: trait_args.clone(),
            trait_const_args: trait_const_args.clone(),
        });
    }
    match trait_id {
        TraitId::Builtin(trait_id) => {
            for supertrait in trait_id.supertraits() {
                let supertrait_args = if supertrait.preserves_trait_args {
                    trait_args.clone()
                } else {
                    Vec::new()
                };
                push_trait_goal_assumption_with_supertraits_inner(
                    module,
                    interner,
                    trait_signatures,
                    self_ty,
                    TraitId::Builtin(supertrait.trait_id),
                    supertrait_args,
                    Vec::new(),
                    assumptions,
                    visited,
                );
            }
        }
        TraitId::Source(trait_id) => {
            let Some(trait_signature) = trait_signatures.get(&trait_id).copied() else {
                return;
            };
            let substitutions = trait_signature
                .signature
                .generics
                .iter()
                .zip(&trait_args)
                .map(|(generic, arg)| (generic.clone(), *arg))
                .collect::<HashMap<_, _>>();
            let const_substitutions =
                const_substitutions_from_self_describing_args(&trait_const_args);
            for supertrait in &trait_signature.signature.supertraits {
                let supertrait = substitute_imported_type(
                    interner,
                    module,
                    trait_signature.interner,
                    supertrait.ty,
                    &substitutions,
                    &const_substitutions,
                    None,
                );
                let Some((supertrait_id, supertrait_args, supertrait_const_args)) =
                    trait_id_and_args(interner, supertrait)
                else {
                    continue;
                };
                push_trait_goal_assumption_with_supertraits_inner(
                    module,
                    interner,
                    trait_signatures,
                    self_ty,
                    supertrait_id,
                    supertrait_args,
                    supertrait_const_args,
                    assumptions,
                    visited,
                );
            }
        }
    }
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
            | TyKind::VolatilePointer { .. }
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

pub(crate) struct VisibleExtensionsInput<'a> {
    pub module_id: nia_ids::ModuleId,
    pub graph: &'a nia_imports::ModuleGraph,
    pub using_scope: &'a nia_defs::ModuleUsingScope,
    pub using_scopes: &'a HashMap<nia_ids::ModuleId, nia_defs::ModuleUsingScope>,
    pub public_surfaces: &'a PublicSurfaces,
    pub defs: &'a dyn Fn(nia_ids::ModuleId) -> Option<DefCollection>,
    pub normalizations: TypeNormalizationResolver<'a>,
    pub visible_type_signatures: VisibleTypeSignatures<'a>,
    pub extensions: &'a ExtensionMethods,
    pub associated_values: &'a ExtensionAssociatedValues,
    pub trait_impls: &'a [ProgramTraitImplSignature],
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct VisibleTypeSignatures<'a> {
    pub type_aliases: &'a HashMap<GlobalDefId, ProgramTypeAliasSignature>,
}

struct VisibleExtensionResolverCache<'a> {
    defs: &'a dyn Fn(nia_ids::ModuleId) -> Option<DefCollection>,
    normalizations: TypeNormalizationResolver<'a>,
    defs_cache: HashMap<nia_ids::ModuleId, Option<DefCollection>>,
    normalization_cache: HashMap<nia_ids::ModuleId, Option<TypeNormalization>>,
}

impl<'a> VisibleExtensionResolverCache<'a> {
    fn new(
        defs: &'a dyn Fn(nia_ids::ModuleId) -> Option<DefCollection>,
        normalizations: TypeNormalizationResolver<'a>,
    ) -> Self {
        Self {
            defs,
            normalizations,
            defs_cache: HashMap::new(),
            normalization_cache: HashMap::new(),
        }
    }

    fn defs(&mut self, module_id: nia_ids::ModuleId) -> Option<&DefCollection> {
        if !self.defs_cache.contains_key(&module_id) {
            self.defs_cache.insert(module_id, (self.defs)(module_id));
        }
        self.defs_cache
            .get(&module_id)
            .and_then(|defs| defs.as_ref())
    }

    fn normalization(&mut self, module_id: nia_ids::ModuleId) -> Option<&TypeNormalization> {
        if !self.normalization_cache.contains_key(&module_id) {
            self.normalization_cache
                .insert(module_id, (self.normalizations)(module_id));
        }
        self.normalization_cache
            .get(&module_id)
            .and_then(|normalization| normalization.as_ref())
    }
}

pub(crate) fn visible_extensions_for_module(
    input: VisibleExtensionsInput<'_>,
) -> VisibleExtensionsForModule {
    let VisibleExtensionsInput {
        module_id,
        graph,
        using_scope,
        using_scopes,
        public_surfaces,
        defs,
        normalizations,
        visible_type_signatures,
        extensions,
        associated_values,
        trait_impls,
    } = input;
    let mut resolver_cache = VisibleExtensionResolverCache::new(defs, normalizations);
    let visibility_context = VisibilityClosureContext {
        module_id,
        graph,
        using_scope,
        using_scopes,
        defs,
        normalizations,
        visible_type_signatures,
        extensions,
        associated_values,
    };
    let visible_modules = declared_module_closure(&visibility_context);
    let witness_modules = declared_witness_module_closure(&visibility_context);
    let Some(current_normalization) = resolver_cache.normalization(module_id).cloned() else {
        return VisibleExtensionsForModule {
            methods: VisibleExtensionMethods::default(),
            interner: TyInterner::default(),
        };
    };
    let mut target_interner = current_normalization.interner.clone();
    let mut visible = VisibleExtensionMethods::default();
    let extension_visibility_allows = |visibility, defining_module| {
        nia_imports::visibility_allows(visibility, graph, defining_module, module_id)
    };
    extensions.for_each_visible_method(
        module_id,
        visible_modules.iter().copied(),
        extension_visibility_allows,
        |method| {
            if !resolver_cache
                .defs(method.def_id.module_id)
                .is_some_and(|defs| defs.defs.get(method.def_id.def_id).is_some())
            {
                return;
            }
            let trait_is_visible = method.trait_id.is_some_and(|trait_id| {
                witness_modules.contains(&method.def_id.module_id)
                    && trait_id_is_visible(
                        module_id,
                        &witness_modules,
                        trait_id,
                        public_surfaces,
                        &mut resolver_cache,
                    )
            });
            let Some(method_normalization) = resolver_cache.normalization(method.def_id.module_id)
            else {
                return;
            };
            let target_ty = method_normalization.normalize(method.target_ty);
            let target_ty = import_type_into(
                &mut target_interner,
                &method_normalization.interner,
                target_ty,
            );
            visible.insert(
                method.impl_id,
                target_ty,
                VisibleExtensionMethod {
                    name: method.name.clone(),
                    def_id: method.def_id,
                    impl_id: method.impl_id,
                    effective_generics: method.effective_generics.clone(),
                    trait_id: method.trait_id,
                    trait_args: method
                        .trait_args
                        .iter()
                        .map(|arg| {
                            let arg = method_normalization.normalize(*arg);
                            import_type_into(
                                &mut target_interner,
                                &method_normalization.interner,
                                arg,
                            )
                        })
                        .collect(),
                    where_predicates: import_where_predicates(
                        &mut target_interner,
                        &method_normalization.interner,
                        &method.where_predicates,
                    ),
                    is_callable: extension_visibility_allows(
                        method.visibility,
                        method.def_id.module_id,
                    ),
                    is_trait_witness: trait_is_visible,
                },
            );
        },
    );
    for impl_signature in trait_impls {
        if !witness_modules.contains(&impl_signature.module_id) {
            continue;
        }
        if trait_id_is_visible(
            module_id,
            &witness_modules,
            impl_signature.trait_id,
            public_surfaces,
            &mut resolver_cache,
        ) {
            visible.insert_trait_witness_impl(impl_signature.module_id, impl_signature.impl_id);
        }
    }
    associated_values.for_each_visible_value(
        module_id,
        visible_modules.iter().copied(),
        extension_visibility_allows,
        |value| {
            if !resolver_cache
                .defs(value.def_id.module_id)
                .is_some_and(|defs| defs.defs.get(value.def_id.def_id).is_some())
            {
                return;
            }
            let Some(value_normalization) = resolver_cache.normalization(value.def_id.module_id)
            else {
                return;
            };
            let target_ty = value_normalization.normalize(value.target_ty);
            let target_ty = import_type_into(
                &mut target_interner,
                &value_normalization.interner,
                target_ty,
            );
            visible.insert_associated_value(
                value.impl_id,
                target_ty,
                VisibleExtensionAssociatedValue {
                    name: value.name.clone(),
                    def_id: value.def_id,
                },
            );
        },
    );
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
    resolver_cache: &mut VisibleExtensionResolverCache<'_>,
) -> bool {
    let TraitId::Source(trait_id) = trait_id else {
        return true;
    };
    if trait_id.module_id == current_module {
        return true;
    }
    if imported_modules.contains(&trait_id.module_id) {
        return resolver_cache.defs(trait_id.module_id).is_some_and(|defs| {
            defs.defs
                .get(trait_id.def_id)
                .is_some_and(|def| def.visibility == Visibility::Public)
        });
    }
    std::iter::once(current_module)
        .chain(imported_modules.iter().copied())
        .any(|module_id| public_surface_exports_type(public_surfaces, module_id, trait_id))
}

fn public_surface_exports_type(
    public_surfaces: &PublicSurfaces,
    module_id: nia_ids::ModuleId,
    def_id: GlobalDefId,
) -> bool {
    public_surfaces.get(module_id).is_some_and(|surface| {
        surface.types.values().any(|item| {
            item.target_module == def_id.module_id
                && item.target_def_id == def_id.def_id
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

fn declared_module_closure(context: &VisibilityClosureContext<'_>) -> Vec<nia_ids::ModuleId> {
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    enqueue_using_scope_modules(context.using_scope, &mut queue);
    enqueue_public_inherent_extension_providers_for_using_scope(
        context,
        context.using_scope,
        &mut queue,
    );

    while let Some(visible) = queue.pop_front() {
        if visible == context.module_id || !seen.insert(visible) {
            continue;
        }
        if let Some(using_scope) = context.using_scopes.get(&visible) {
            enqueue_using_scope_modules(using_scope, &mut queue);
            enqueue_public_inherent_extension_providers_for_using_scope(
                context,
                using_scope,
                &mut queue,
            );
        }
    }

    let mut modules = seen.into_iter().collect::<Vec<_>>();
    modules.sort();
    modules
}

fn declared_witness_module_closure(
    context: &VisibilityClosureContext<'_>,
) -> Vec<nia_ids::ModuleId> {
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    enqueue_using_scope_modules(context.using_scope, &mut queue);
    enqueue_public_inherent_extension_providers_for_using_scope(
        context,
        context.using_scope,
        &mut queue,
    );

    while let Some(module_id) = queue.pop_front() {
        if module_id == context.module_id || !seen.insert(module_id) {
            continue;
        }
        if let Some(using_scope) = context.using_scopes.get(&module_id) {
            enqueue_using_scope_modules(using_scope, &mut queue);
            enqueue_public_inherent_extension_providers_for_using_scope(
                context,
                using_scope,
                &mut queue,
            );
        }
    }

    let mut modules = seen.into_iter().collect::<Vec<_>>();
    modules.sort();
    modules
}

fn enqueue_using_scope_modules(
    using_scope: &nia_defs::ModuleUsingScope,
    queue: &mut VecDeque<nia_ids::ModuleId>,
) {
    queue.extend(using_scope.modules.values().copied());
    queue.extend(using_scope.types.values().map(|entry| entry.target_module));
}

struct VisibilityClosureContext<'a> {
    module_id: nia_ids::ModuleId,
    graph: &'a nia_imports::ModuleGraph,
    using_scope: &'a nia_defs::ModuleUsingScope,
    using_scopes: &'a HashMap<nia_ids::ModuleId, nia_defs::ModuleUsingScope>,
    defs: &'a dyn Fn(nia_ids::ModuleId) -> Option<DefCollection>,
    normalizations: TypeNormalizationResolver<'a>,
    visible_type_signatures: VisibleTypeSignatures<'a>,
    extensions: &'a ExtensionMethods,
    associated_values: &'a ExtensionAssociatedValues,
}

fn enqueue_public_inherent_extension_providers_for_using_scope(
    context: &VisibilityClosureContext<'_>,
    using_scope: &nia_defs::ModuleUsingScope,
    queue: &mut VecDeque<nia_ids::ModuleId>,
) {
    for type_def_id in using_scope
        .types
        .values()
        .filter(|entry| entry.namespace == PublicNamespace::Type)
        .filter_map(|entry| {
            nominal_def_id_for_public_type(
                GlobalDefId {
                    module_id: entry.target_module,
                    def_id: entry.target_def_id,
                },
                context.defs,
                context.normalizations,
                context.visible_type_signatures,
            )
        })
    {
        queue.extend(public_inherent_method_providers_for_nominal(
            context.module_id,
            context.graph,
            type_def_id,
            context.extensions,
        ));
        queue.extend(public_associated_value_providers_for_nominal(
            context.module_id,
            context.graph,
            type_def_id,
            context.associated_values,
        ));
    }
}

fn nominal_def_id_for_public_type(
    def_id: GlobalDefId,
    defs: &dyn Fn(nia_ids::ModuleId) -> Option<DefCollection>,
    normalizations: TypeNormalizationResolver<'_>,
    visible_type_signatures: VisibleTypeSignatures<'_>,
) -> Option<GlobalDefId> {
    let defs = defs(def_id.module_id)?;
    let def = defs.defs.get(def_id.def_id)?;
    if matches!(
        def.kind,
        nia_defs::DefKind::Struct | nia_defs::DefKind::Union | nia_defs::DefKind::Enum
    ) {
        return Some(def_id);
    }
    if def.kind != nia_defs::DefKind::TypeAlias {
        return None;
    }
    let normalization = normalizations(def_id.module_id)?;
    let alias = visible_type_signatures.type_aliases.get(&def_id)?;
    let normalized = normalization.normalize(alias.signature.target);
    match normalization.interner.get(normalized) {
        Some(TyKind::Nominal { def_id, .. }) => Some(*def_id),
        _ => None,
    }
}

fn public_inherent_method_providers_for_nominal(
    module_id: nia_ids::ModuleId,
    graph: &nia_imports::ModuleGraph,
    target_def_id: GlobalDefId,
    extensions: &ExtensionMethods,
) -> Vec<nia_ids::ModuleId> {
    let mut providers = Vec::new();
    for method in extensions.methods_for_nominal_target(target_def_id) {
        if method.trait_id.is_some()
            || !nia_imports::visibility_allows(
                method.visibility,
                graph,
                method.def_id.module_id,
                module_id,
            )
        {
            continue;
        }
        providers.push(method.def_id.module_id);
    }
    providers
}

fn public_associated_value_providers_for_nominal(
    module_id: nia_ids::ModuleId,
    graph: &nia_imports::ModuleGraph,
    target_def_id: GlobalDefId,
    associated_values: &ExtensionAssociatedValues,
) -> Vec<nia_ids::ModuleId> {
    let mut providers = Vec::new();
    for value in associated_values.values_for_nominal_target(target_def_id) {
        if !nia_imports::visibility_allows(
            value.visibility,
            graph,
            value.def_id.module_id,
            module_id,
        ) {
            continue;
        }
        providers.push(value.def_id.module_id);
    }
    providers
}

fn nominal_target_def_id(interner: &TyInterner, ty: InternedTyId) -> Option<GlobalDefId> {
    match interner.get(ty) {
        Some(TyKind::Nominal { def_id, .. }) => Some(*def_id),
        _ => None,
    }
}
