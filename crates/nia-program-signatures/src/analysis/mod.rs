// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_defs::{
    AssociatedTypeBindingSignature, DefCollection, ExtensionAssociatedValue,
    ExtensionAssociatedValues, ExtensionMethod, ExtensionMethods, WhereBoundSignature,
    WherePredicateSignature,
};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{
    BuiltinAssociatedType, BuiltinTrait, BuiltinTraitMethod, GlobalDefId, InternedTyId,
    ReceiverKind, TraitImplId, Visibility,
};
use nia_item_signatures::{
    FunctionSignature, ItemSignatures, ProgramComptimeSignature, ProgramEnumSignature,
    ProgramFunctionSignature, ProgramGlobalSignature, ProgramStructSignature,
    ProgramTraitImplSignature, ProgramTraitSignature, ProgramTypeAliasSignature,
    ProgramUnionSignature, TraitImplSignature, TraitSignature,
};
use nia_symbol::{SymbolId, ToSymbolId, symbol_text_or_unresolved};
use nia_symbol_table::SymbolTable;
use nia_trait_solve::{
    AssociatedTypeProjectionEq, IntrinsicOverlap, TraitGoal, TraitSolverContext,
};
use nia_ty::{PrimitiveTy, TraitId, TyInterner, TyKind, import_type_into};
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;

mod facts;
#[cfg(test)]
mod tests;
mod types;
mod visible;

pub use facts::*;
use types::*;
pub use visible::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NominalExtensionProviderEntry {
    pub target: GlobalDefId,
    pub module_id: nia_ids::ModuleId,
    pub visibility: Visibility,
}

pub struct ModuleSignatureInput<'a> {
    pub module_id: nia_ids::ModuleId,
    pub defs: &'a DefCollection,
    pub lowering: &'a TypeLowering,
    pub signatures: &'a ItemSignatures,
}

pub struct ExtensionModuleInput<'a> {
    pub module_id: nia_ids::ModuleId,
    pub defs: &'a DefCollection,
    pub lowering: &'a TypeLowering,
    pub signatures: &'a ItemSignatures,
    pub function_signatures: &'a ItemSignatures,
    pub type_signatures: &'a ItemSignatures,
    pub normalization: &'a TypeNormalization,
}

pub struct ExtensionMethodIndexModuleInput<'a> {
    pub module_id: nia_ids::ModuleId,
    pub defs: &'a DefCollection,
    pub lowering: &'a TypeLowering,
    pub signatures: &'a ItemSignatures,
    pub normalization: &'a TypeNormalization,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionTraitSignatureIndex {
    pub trait_defs: HashSet<GlobalDefId>,
    pub trait_signatures: HashMap<GlobalDefId, ProgramTraitSignature>,
}

pub struct ExtensionMethodValidationInput<'a> {
    pub trait_defs: &'a HashSet<GlobalDefId>,
    pub trait_signatures: &'a HashMap<GlobalDefId, ProgramTraitSignature>,
    pub trait_impls_for_trait: &'a dyn Fn(TraitId) -> Vec<ProgramTraitImplSignature>,
    pub symbols: &'a SymbolTable,
}

fn symbol_name(symbols: &SymbolTable, symbol: SymbolId) -> String {
    symbol_text_or_unresolved(symbols, symbol)
}

pub fn collect_valid_program_trait_impls(
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

pub fn collect_invalid_trait_impl_method_ids(
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

pub fn collect_extension_methods_for_module(
    module: &ExtensionModuleInput<'_>,
    input: ExtensionMethodValidationInput<'_>,
) -> (ExtensionMethods, Vec<Diagnostic>) {
    let mut extensions = ExtensionMethods::default();
    let mut diagnostics = Vec::new();
    validate_supertraits(module, input.trait_defs, &mut diagnostics);
    for impl_signature in &module.signatures.trait_impls {
        if impl_signature.builtin.is_some() {
            continue;
        }
        let target_ty = module.normalization.normalize(impl_signature.target_ty);
        if !is_extendable_target(&module.normalization.interner, target_ty) {
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                impl_signature.span,
                "extend target must be an extendable value type",
            ));
            continue;
        }
        let trait_id = impl_trait_id(module, impl_signature, input.trait_defs, &mut diagnostics);
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
                input.trait_signatures,
                input.trait_impls_for_trait,
                input.symbols,
                &mut diagnostics,
            ),
            Some(TraitId::Builtin(trait_id)) => validate_builtin_trait_impl(
                module,
                impl_signature,
                target_ty,
                trait_id,
                input.trait_impls_for_trait,
                input.symbols,
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
    (extensions, diagnostics)
}

pub fn collect_extension_method_index_for_module(
    module: &ExtensionMethodIndexModuleInput<'_>,
    defs: &dyn ProgramDefsResolver,
) -> ExtensionMethods {
    let mut extensions = ExtensionMethods::default();
    for impl_signature in &module.signatures.trait_impls {
        let target_ty = module.normalization.normalize(impl_signature.target_ty);
        if !is_extendable_target(&module.normalization.interner, target_ty) {
            continue;
        }
        let trait_id = impl_trait_id_for_index_with_defs(module, impl_signature, defs);
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
    extensions
}

pub fn collect_nominal_extension_providers_for_module(
    module: &ExtensionMethodIndexModuleInput<'_>,
    defs: &dyn ProgramDefsResolver,
) -> Vec<NominalExtensionProviderEntry> {
    let mut providers = Vec::new();
    for impl_signature in &module.signatures.trait_impls {
        let target_ty = module.normalization.normalize(impl_signature.target_ty);
        if !is_extendable_target(&module.normalization.interner, target_ty) {
            continue;
        }
        let Some(target) = nominal_target_def_id(&module.normalization.interner, target_ty) else {
            continue;
        };
        let trait_id = impl_trait_id_for_index_with_defs(module, impl_signature, defs);
        if trait_id.is_none() {
            providers.extend(impl_signature.methods.iter().map(|method| {
                NominalExtensionProviderEntry {
                    target,
                    module_id: module.module_id,
                    visibility: method.visibility,
                }
            }));
        }
        providers.extend(impl_signature.associated_values.iter().map(|value| {
            NominalExtensionProviderEntry {
                target,
                module_id: module.module_id,
                visibility: value.visibility,
            }
        }));
    }
    providers.sort_by_key(|provider| {
        (
            provider.target,
            provider.module_id,
            visibility_rank(provider.visibility),
        )
    });
    providers.dedup();
    providers
}

fn visibility_rank(visibility: Visibility) -> u8 {
    match visibility {
        Visibility::Private => 0,
        Visibility::PublicSuper => 1,
        Visibility::PublicPkg => 2,
        Visibility::Public => 3,
    }
}

fn extension_method_effective_generics(
    module: &impl ExtensionMethodModule,
    impl_signature: &TraitImplSignature,
    method: &nia_item_signatures::TraitImplMethodSignature,
    _target_ty: InternedTyId,
) -> Vec<SymbolId> {
    let mut generics = impl_signature.generics.clone();
    if let Some(def) = module.defs().defs.get(method.def_id) {
        generics.extend(def.generics.iter().cloned());
    }
    generics
}

trait ExtensionMethodModule {
    fn defs(&self) -> &DefCollection;
}

impl ExtensionMethodModule for ExtensionModuleInput<'_> {
    fn defs(&self) -> &DefCollection {
        self.defs
    }
}

impl ExtensionMethodModule for ExtensionMethodIndexModuleInput<'_> {
    fn defs(&self) -> &DefCollection {
        self.defs
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

pub fn collect_extension_associated_value_index_for_module(
    module: &ExtensionMethodIndexModuleInput<'_>,
) -> (ExtensionAssociatedValues, Vec<Diagnostic>) {
    let mut values = ExtensionAssociatedValues::default();
    let mut diagnostics = Vec::new();
    for impl_signature in &module.signatures.trait_impls {
        let target_ty = module.normalization.normalize(impl_signature.target_ty);
        if !is_extendable_target(&module.normalization.interner, target_ty) {
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
    (values, diagnostics)
}

fn impl_trait_id(
    module: &ExtensionModuleInput<'_>,
    impl_signature: &TraitImplSignature,
    trait_defs: &HashSet<GlobalDefId>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<TraitId> {
    let Some(trait_ty) = impl_signature.trait_ty else {
        return None;
    };
    let span = impl_signature.trait_span.unwrap_or(impl_signature.span);
    let ty = module.normalization.normalize(trait_ty);
    match module.lowering.interner.get(ty).cloned() {
        Some(TyKind::Nominal { def_id, .. }) => {
            if !trait_defs.contains(&def_id) {
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

fn impl_trait_id_for_index_with_defs(
    module: &ExtensionMethodIndexModuleInput<'_>,
    impl_signature: &TraitImplSignature,
    defs: &dyn ProgramDefsResolver,
) -> Option<TraitId> {
    let trait_ty = impl_signature.trait_ty?;
    let ty = module.normalization.normalize(trait_ty);
    match module.lowering.interner.get(ty).cloned() {
        Some(TyKind::Nominal { def_id, .. }) => defs.defs(def_id.module_id).and_then(|defs| {
            matches!(
                defs.defs.get(def_id.def_id).map(|def| def.kind),
                Some(nia_defs::DefKind::Trait)
            )
            .then_some(TraitId::Source(def_id))
        }),
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
    name: SymbolId,
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

fn trait_signature_ref(
    trait_signatures: &HashMap<GlobalDefId, ProgramTraitSignature>,
    trait_id: GlobalDefId,
) -> Option<TraitSignatureRef<'_>> {
    trait_signatures
        .get(&trait_id)
        .map(|signature| TraitSignatureRef {
            signature: &signature.signature,
            interner: &signature.interner,
        })
}

fn validate_supertraits(
    module: &ExtensionModuleInput<'_>,
    trait_defs: &HashSet<GlobalDefId>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for trait_signature in module.signatures.traits.values() {
        for supertrait in &trait_signature.supertraits {
            let _ = supertrait_id(
                module,
                supertrait.ty,
                supertrait.span,
                trait_defs,
                diagnostics,
            );
        }
    }
}

fn supertrait_id(
    module: &ExtensionModuleInput<'_>,
    ty: InternedTyId,
    span: nia_span::Span,
    trait_defs: &HashSet<GlobalDefId>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<TraitId> {
    let ty = module.normalization.normalize(ty);
    match module.lowering.interner.get(ty).cloned() {
        Some(TyKind::Nominal { def_id, .. }) => {
            if !trait_defs.contains(&def_id) {
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
    trait_signatures: &HashMap<GlobalDefId, ProgramTraitSignature>,
    trait_impls_for_trait: &dyn Fn(TraitId) -> Vec<ProgramTraitImplSignature>,
    symbols: &SymbolTable,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let Some(trait_signature) = trait_signature_ref(trait_signatures, trait_id) else {
        return false;
    };
    let start_len = diagnostics.len();
    let (trait_args, trait_const_args) =
        impl_trait_args_and_consts(module, impl_signature, Some(TraitId::Source(trait_id)))
            .unwrap_or_default();
    for associated_type in &impl_signature.associated_types {
        if !trait_signature
            .signature
            .associated_types
            .iter()
            .any(|required| required.name == associated_type.name)
        {
            let name = symbol_name(symbols, associated_type.name);
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                associated_type.span,
                format!("associated type `{name}` is not a member of implemented trait"),
            ));
        }
    }
    for associated_value in &impl_signature.associated_values {
        let Some(required) = trait_signature
            .signature
            .associated_values
            .iter()
            .find(|required| required.name == associated_value.name)
        else {
            let name = symbol_name(symbols, associated_value.name);
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                associated_value.span,
                format!("associated comptime `{name}` is not a member of implemented trait"),
            ));
            continue;
        };
        let Some(actual_ty) = module
            .signatures
            .comptimes
            .get(&associated_value.def_id)
            .and_then(|signature| signature.explicit_type)
        else {
            let name = symbol_name(symbols, associated_value.name);
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                associated_value.span,
                format!(
                    "associated comptime `{name}` requires an explicit type to satisfy the trait requirement"
                ),
            ));
            continue;
        };
        if !trait_associated_comptime_type_matches(TraitAssociatedComptimeTypeMatch {
            module,
            trait_signature,
            required_ty: required.ty,
            actual_ty,
            target_ty,
            trait_id,
            trait_args: &trait_args,
            trait_const_args: &trait_const_args,
            impl_signature,
        }) {
            let name = symbol_name(symbols, associated_value.name);
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                associated_value.span,
                format!(
                    "implementation of associated comptime `{name}` does not match the trait requirement"
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
            let name = symbol_name(symbols, required.name);
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                impl_signature.span,
                format!("missing definition for associated type `{name}`"),
            ));
        }
    }
    for required in &trait_signature.signature.associated_values {
        if !impl_signature
            .associated_values
            .iter()
            .any(|associated_value| associated_value.name == required.name)
        {
            let name = symbol_name(symbols, required.name);
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                impl_signature.span,
                format!("missing definition for associated comptime `{name}`"),
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
            let name = symbol_name(symbols, method.name);
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                method.span,
                format!("method `{name}` is not a member of implemented trait"),
            ));
        }
    }
    validate_supertrait_impls(
        module,
        impl_signature,
        target_ty,
        trait_signature,
        &trait_args,
        trait_impls_for_trait,
        symbols,
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
                let name = symbol_name(symbols, required.name);
                diagnostics.push(Diagnostic::user_error_at(
                    codes::NAME_RESOLUTION,
                    impl_signature.span,
                    format!("missing implementation for trait method `{name}`"),
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
        let validation_trait_impls = trait_impls_for_trait_goal_and_supertraits(
            module,
            &mut comparison_interner,
            target_ty,
            TraitId::Source(trait_id),
            &trait_args,
            &trait_const_args,
            &trait_signatures,
            trait_impls_for_trait,
        );
        if !trait_method_signature_matches(
            module,
            &validation_trait_impls,
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
            let name = symbol_name(symbols, required.name);
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                method.span,
                format!(
                    "implementation of trait method `{name}` does not match the trait signature"
                ),
            ));
        }
    }
    diagnostics.len() == start_len
}

struct TraitAssociatedComptimeTypeMatch<'a> {
    module: &'a ExtensionModuleInput<'a>,
    trait_signature: TraitSignatureRef<'a>,
    required_ty: nia_ids::InternedTyId,
    actual_ty: nia_ids::InternedTyId,
    target_ty: nia_ids::InternedTyId,
    trait_id: GlobalDefId,
    trait_args: &'a [nia_ids::InternedTyId],
    trait_const_args: &'a [nia_ty::ConstGenericArg],
    impl_signature: &'a TraitImplSignature,
}

fn trait_associated_comptime_type_matches(input: TraitAssociatedComptimeTypeMatch<'_>) -> bool {
    let mut comparison_interner = input.module.normalization.interner.clone();
    let substitutions = input
        .trait_signature
        .signature
        .generics
        .iter()
        .zip(input.trait_args)
        .map(|(generic, arg)| (generic.clone(), *arg))
        .collect::<HashMap<_, _>>();
    let const_substitutions = const_substitutions_from_self_describing_args(input.trait_const_args);
    let projection_context = Some(ProjectionImplContext {
        trait_id: input.trait_id,
        trait_args: input.trait_args,
        trait_const_args: input.trait_const_args,
        self_ty: input.target_ty,
        associated_types: &input.impl_signature.associated_types,
    });
    let required = substitute_imported_type(
        &mut comparison_interner,
        input.module,
        input.trait_signature.interner,
        input.required_ty,
        &substitutions,
        &const_substitutions,
        projection_context,
        Some(input.target_ty),
    );
    let actual = substitute_imported_type(
        &mut comparison_interner,
        input.module,
        &input.module.lowering.interner,
        input.actual_ty,
        &HashMap::new(),
        &HashMap::new(),
        projection_context,
        None,
    );
    types_equivalent_in_interner(&comparison_interner, required, actual)
}

fn validate_builtin_trait_impl(
    module: &ExtensionModuleInput<'_>,
    impl_signature: &TraitImplSignature,
    target_ty: nia_ids::InternedTyId,
    trait_id: BuiltinTrait,
    trait_impls_for_trait: &dyn Fn(TraitId) -> Vec<ProgramTraitImplSignature>,
    symbols: &SymbolTable,
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
        if !trait_id
            .associated_types()
            .iter()
            .any(|expected| expected.symbol_id() == associated_type.name)
        {
            let name = symbol_name(symbols, associated_type.name);
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                associated_type.span,
                format!("associated type `{name}` is not a member of implemented trait"),
            ));
        }
    }
    for associated_type_name in trait_id.associated_types().iter().copied() {
        if !impl_signature
            .associated_types
            .iter()
            .any(|associated_type| associated_type.name == associated_type_name.symbol_id())
        {
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                impl_signature.span,
                format!(
                    "missing definition for associated type `{}`",
                    associated_type_name.name()
                ),
            ));
        }
    }
    validate_builtin_supertrait_impls(
        module,
        impl_signature,
        target_ty,
        trait_id,
        trait_impls_for_trait,
        diagnostics,
    );
    let expected_methods = trait_id.required_methods();
    for method in &impl_signature.methods {
        if !expected_methods
            .iter()
            .any(|expected_method| expected_method.symbol_id() == method.name)
        {
            let name = symbol_name(symbols, method.name);
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                method.span,
                format!("method `{name}` is not a member of implemented trait"),
            ));
        }
    }
    for expected_method in expected_methods {
        let matching_methods = impl_signature
            .methods
            .iter()
            .filter(|method| method.name == expected_method.symbol_id())
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
    trait_impls_for_trait: &dyn Fn(TraitId) -> Vec<ProgramTraitImplSignature>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for supertrait in trait_id.supertraits() {
        let supertrait_args = if supertrait.preserves_trait_args {
            builtin_impl_trait_args(module, impl_signature, trait_id).unwrap_or_default()
        } else {
            Vec::new()
        };
        let supertrait_id = TraitId::Builtin(supertrait.trait_id);
        let trait_impls = trait_impls_for_trait(supertrait_id);
        if !has_matching_trait_impl(
            &module.lowering.interner,
            target_ty,
            supertrait_id,
            &supertrait_args,
            &trait_impls,
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
        BuiltinTrait::Deref | BuiltinTrait::DerefMut => BuiltinAssociatedType::Target.symbol_id(),
        BuiltinTrait::Index | BuiltinTrait::IndexMut => BuiltinAssociatedType::Output.symbol_id(),
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
    let Some(output) =
        associated_type_ty(impl_signature, BuiltinAssociatedType::Output.symbol_id())
    else {
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
    let Some(item) = associated_type_ty(impl_signature, BuiltinAssociatedType::Item.symbol_id())
    else {
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
    let Some(iter) = associated_type_ty(impl_signature, BuiltinAssociatedType::Iter.symbol_id())
    else {
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
    let Some(output) =
        associated_type_ty(impl_signature, BuiltinAssociatedType::Output.symbol_id())
    else {
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
    trait_impls_for_trait: &dyn Fn(TraitId) -> Vec<ProgramTraitImplSignature>,
    symbols: &SymbolTable,
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
            def_id: supertrait_def_id,
            args: supertrait_args,
            ..
        }) = comparison_interner.get(supertrait).cloned()
        else {
            continue;
        };
        let supertrait_id = TraitId::Source(supertrait_def_id);
        let trait_impls = trait_impls_for_trait(supertrait_id);
        if !has_matching_trait_impl(
            &comparison_interner,
            target_ty,
            supertrait_id,
            &supertrait_args,
            &trait_impls,
        ) {
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                impl_signature.span,
                format!(
                    "implementation of trait requires explicit implementation of supertrait `{}`",
                    trait_name(module, supertrait_def_id, symbols)
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
    trait_generics: &[SymbolId],
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

fn trait_name(
    module: &ExtensionModuleInput<'_>,
    trait_id: GlobalDefId,
    symbols: &SymbolTable,
) -> String {
    module
        .defs
        .defs
        .get(trait_id.def_id)
        .filter(|_| trait_id.module_id == module.module_id)
        .map(|def| symbol_name(symbols, def.name))
        .unwrap_or_else(|| format!("trait#{}.{}", trait_id.module_id.0, trait_id.def_id.0))
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
    trait_signatures: &HashMap<GlobalDefId, ProgramTraitSignature>,
    required: &nia_item_signatures::FunctionSignature,
    actual: &nia_item_signatures::FunctionSignature,
) -> bool {
    let mut assumptions = Vec::new();
    push_trait_goal_assumption_with_supertraits(
        module,
        interner,
        trait_signatures,
        self_ty,
        trait_id,
        trait_args.to_vec(),
        trait_const_args.to_vec(),
        &mut assumptions,
    );
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
        trait_impl_index: None,
        layouts: None,
        local_module_id: module.module_id,
        local_enums: &module.signatures.enums,
        program_is_enum: None,
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

fn trait_impls_for_trait_goal_and_supertraits(
    module: &ExtensionModuleInput<'_>,
    interner: &mut TyInterner,
    self_ty: InternedTyId,
    trait_id: TraitId,
    trait_args: &[InternedTyId],
    trait_const_args: &[nia_ty::ConstGenericArg],
    trait_signatures: &HashMap<GlobalDefId, ProgramTraitSignature>,
    trait_impls_for_trait: &dyn Fn(TraitId) -> Vec<ProgramTraitImplSignature>,
) -> Vec<ProgramTraitImplSignature> {
    let mut goals = Vec::new();
    push_trait_goal_assumption_with_supertraits(
        module,
        interner,
        trait_signatures,
        self_ty,
        trait_id,
        trait_args.to_vec(),
        trait_const_args.to_vec(),
        &mut goals,
    );
    let mut seen = HashSet::new();
    goals
        .into_iter()
        .filter_map(|goal| seen.insert(goal.trait_id).then_some(goal.trait_id))
        .flat_map(trait_impls_for_trait)
        .collect()
}

fn push_where_predicate_solver_assumptions(
    module: &ExtensionModuleInput<'_>,
    interner: &mut TyInterner,
    predicates: &[WherePredicateSignature],
    trait_signatures: &HashMap<GlobalDefId, ProgramTraitSignature>,
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
    trait_signatures: &HashMap<GlobalDefId, ProgramTraitSignature>,
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
    trait_signatures: &HashMap<GlobalDefId, ProgramTraitSignature>,
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
            let Some(trait_signature) = trait_signature_ref(trait_signatures, trait_id) else {
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
                    Some(self_ty),
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
            | TyKind::BuiltinType(_)
            | TyKind::BuiltinTrait { .. }
            | TyKind::SlicePointee { .. }
            | TyKind::TraitObject { .. }
            | TyKind::TraitObjectPointee { .. }
            | TyKind::Projection { .. }
            | TyKind::Range { .. }
            | TyKind::Optional { .. }
            | TyKind::ErrorUnion { .. }
            | TyKind::GenericParam(_)
            | TyKind::SelfParam,
        ) => true,
    }
}

fn nominal_target_def_id(interner: &TyInterner, ty: InternedTyId) -> Option<GlobalDefId> {
    match interner.get(ty) {
        Some(TyKind::Nominal { def_id, .. }) => Some(*def_id),
        _ => None,
    }
}
