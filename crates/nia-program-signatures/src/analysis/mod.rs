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
    FunctionSignature, GenericParamSignature, GenericParamSignatureKind, ItemSignatures,
    ProgramConstSignature, ProgramEnumSignature, ProgramFunctionSignature, ProgramGlobalSignature,
    ProgramStructSignature, ProgramTraitImplSignature, ProgramTraitSignature,
    ProgramTypeAliasSignature, ProgramUnionSignature, TraitImplSignature, TraitSignature,
    generic_argument_substitutions,
};
use nia_symbol::{SymbolId, SymbolMap, ToSymbolId, symbol_text_or_unresolved};
use nia_symbol_table::SymbolTable;
use nia_trait_solve::{
    AssociatedTypeProjectionEq, IntrinsicOverlap, TraitGoal, TraitSolverContext,
};
use nia_ty::{PrimitiveTy, TraitId, TyKind, TypeStore, TypeStoreAppend};
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

/// A module that can provide inherent extensions for a nominal target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NominalExtensionProviderEntry {
    /// Nominal type definition receiving the extension.
    pub target: GlobalDefId,
    /// Module containing the extension declaration.
    pub module_id: nia_ids::ModuleId,
    /// Visibility of the extension member.
    pub visibility: Visibility,
}

/// Minimal module inputs used to qualify item signatures with global ids.
pub struct ModuleSignatureInput<'a> {
    /// Module being qualified.
    pub module_id: nia_ids::ModuleId,
    /// Type store owning the signature type ids.
    pub type_store: &'a TypeStore,
    /// Module-local declaration signatures.
    pub signatures: &'a ItemSignatures,
}

/// All semantic inputs required to validate and collect extensions.
pub struct ExtensionModuleInput<'a> {
    /// Module containing the extension declarations.
    pub module_id: nia_ids::ModuleId,
    /// Type store used to inspect and normalize targets.
    pub type_store: &'a TypeStore,
    /// Definition table for method generic metadata.
    pub defs: &'a DefCollection,
    /// Lowering context for extension type arguments.
    pub lowering: &'a TypeLowering,
    /// Complete module-local signatures.
    pub signatures: &'a ItemSignatures,
    /// Function signatures used by extension validation.
    pub function_signatures: &'a ItemSignatures,
    /// Type signatures used by extension validation.
    pub type_signatures: &'a ItemSignatures,
    /// Canonical normalization for module-owned types.
    pub normalization: &'a TypeNormalization,
}

/// Inputs needed to build a module's extension-method index.
pub struct ExtensionMethodIndexModuleInput<'a> {
    /// Module containing the extension declarations.
    pub module_id: nia_ids::ModuleId,
    /// Type store used to inspect targets.
    pub type_store: &'a TypeStore,
    /// Definition table for method metadata.
    pub defs: &'a DefCollection,
    /// Type-lowering context.
    pub lowering: &'a TypeLowering,
    /// Module-local signatures.
    pub signatures: &'a ItemSignatures,
    /// Canonical normalization for module-owned types.
    pub normalization: &'a TypeNormalization,
}

/// Trait declarations available while validating extension implementations.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionTraitSignatureIndex {
    /// Global ids of known traits.
    pub trait_defs: HashSet<GlobalDefId>,
    /// Program signatures keyed by global trait id.
    pub trait_signatures: HashMap<GlobalDefId, ProgramTraitSignature>,
}

/// Context shared by extension-method validation routines.
#[derive(Clone, Copy)]
pub struct ExtensionMethodValidationInput<'a> {
    /// Type store containing implementation types.
    pub type_store: &'a TypeStore,
    /// Known source-trait definition ids.
    pub trait_defs: &'a HashSet<GlobalDefId>,
    /// Known trait signatures keyed by global id.
    pub trait_signatures: &'a HashMap<GlobalDefId, ProgramTraitSignature>,
    /// Resolver for existing implementations of a trait.
    pub trait_impls_for_trait: &'a dyn Fn(TraitId) -> Vec<ProgramTraitImplSignature>,
    /// Symbol table used to render diagnostics.
    pub symbols: &'a SymbolTable,
}

fn symbol_name(symbols: &SymbolTable, symbol: SymbolId) -> String {
    symbol_text_or_unresolved(symbols, symbol)
}

/// Collects program trait implementations after filtering intrinsic overlaps.
pub fn collect_valid_program_trait_impls(
    modules: &[ExtensionModuleInput<'_>],
) -> Vec<ProgramTraitImplSignature> {
    collect_program_trait_impls(
        &modules
            .iter()
            .map(|module| ModuleSignatureInput {
                module_id: module.module_id,
                type_store: module.type_store,
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

/// Returns method ids belonging to invalid intrinsic-overlap implementations.
pub fn collect_invalid_trait_impl_method_ids(
    modules: &[ExtensionModuleInput<'_>],
) -> HashSet<GlobalDefId> {
    let mut invalid_methods = HashSet::new();
    for module in modules {
        for impl_signature in &module.signatures.trait_impls {
            let target_ty = module.normalization.normalize(impl_signature.target_ty);
            let trait_id = impl_signature.trait_ty.and_then(|trait_ty| {
                trait_id_and_args(module.type_store, trait_ty).map(|(trait_id, _, _)| trait_id)
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
    type_store: &TypeStore,
    ty: nia_ids::InternedTyId,
) -> Option<(
    TraitId,
    Vec<nia_ids::InternedTyId>,
    Vec<nia_ty::ConstGenericArg>,
)> {
    match type_store.get(ty) {
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

/// Validates one module's extension declarations and returns diagnostics.
pub fn collect_extension_method_diagnostics_for_module(
    module: &ExtensionModuleInput<'_>,
    input: ExtensionMethodValidationInput<'_>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    validate_supertraits(module, input, &mut diagnostics);
    for impl_signature in &module.signatures.trait_impls {
        if impl_signature.builtin.is_some() {
            continue;
        }
        let target_ty = module.normalization.normalize(impl_signature.target_ty);
        if !is_extendable_target(module.type_store, target_ty) {
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                impl_signature.span,
                "extend target must be an extendable value type",
            ));
            continue;
        }
        let trait_id = impl_trait_id(module, impl_signature, input.trait_defs, &mut diagnostics);
        if trait_id.is_none() {
            for associated_type in &impl_signature.associated_types {
                diagnostics.push(Diagnostic::user_error_at(
                    codes::NAME_RESOLUTION,
                    associated_type.span,
                    "associated type definitions are only allowed in trait implementations",
                ));
            }
        }
        match trait_id {
            Some(TraitId::Source(trait_id)) => {
                validate_trait_impl(
                    module,
                    impl_signature,
                    target_ty,
                    trait_id,
                    input,
                    &mut diagnostics,
                );
            }
            Some(TraitId::Builtin(trait_id)) => {
                validate_builtin_trait_impl(
                    module,
                    impl_signature,
                    target_ty,
                    trait_id,
                    input.trait_impls_for_trait,
                    input.symbols,
                    &mut diagnostics,
                );
            }
            None => {}
        }
    }
    diagnostics
}

/// Builds the extension-method index for one module.
pub fn collect_extension_method_index_for_module(
    module: &ExtensionMethodIndexModuleInput<'_>,
    trait_defs: &HashSet<GlobalDefId>,
) -> ExtensionMethods {
    let mut extensions = ExtensionMethods::default();
    for impl_signature in &module.signatures.trait_impls {
        let target_ty = module.normalization.normalize(impl_signature.target_ty);
        if !is_extendable_target(module.type_store, target_ty) {
            continue;
        }
        let trait_id = impl_trait_id_for_index(module, impl_signature, trait_defs);
        let trait_args =
            impl_trait_args_for_index(module, impl_signature, trait_id).unwrap_or_default();
        let trait_const_args = impl_signature
            .trait_ty
            .and_then(|trait_ty| trait_id_and_args(module.type_store, trait_ty))
            .map(|(_, _, const_args)| const_args)
            .unwrap_or_default();
        let where_predicates =
            normalize_where_predicates(module.normalization, &impl_signature.where_predicates);
        for method in &impl_signature.methods {
            let effective_generics =
                extension_method_effective_generics(module, impl_signature, method, target_ty);
            let mut effective_const_generics = impl_signature
                .generic_params
                .iter()
                .filter_map(|generic| {
                    matches!(generic.kind, GenericParamSignatureKind::Const { .. })
                        .then_some(generic.name)
                })
                .collect::<Vec<_>>();
            if let Some(def) = module.defs.defs.get(method.def_id) {
                effective_const_generics.extend(def.const_generic_names());
            }
            extensions.insert_with_nominal_target(
                module.module_id,
                ExtensionMethod {
                    name: method.name,
                    def_id: GlobalDefId {
                        module_id: module.module_id,
                        def_id: method.def_id,
                    },
                    impl_id: impl_signature.impl_id,
                    effective_generics,
                    effective_const_generics,
                    target_ty,
                    trait_id,
                    trait_args: trait_args.clone(),
                    trait_const_args: trait_const_args.clone(),
                    where_predicates: where_predicates.clone(),
                    visibility: method.visibility,
                },
                nominal_target_def_id(module.type_store, target_ty),
            );
        }
    }
    extensions
}

/// Collects nominal targets that can provide visible extensions.
pub fn collect_nominal_extension_providers_for_module(
    module: &ExtensionMethodIndexModuleInput<'_>,
    trait_defs: &HashSet<GlobalDefId>,
) -> Vec<NominalExtensionProviderEntry> {
    let mut providers = Vec::new();
    for impl_signature in &module.signatures.trait_impls {
        let target_ty = module.normalization.normalize(impl_signature.target_ty);
        if !is_extendable_target(module.type_store, target_ty) {
            continue;
        }
        let Some(target) = nominal_target_def_id(module.type_store, target_ty) else {
            continue;
        };
        let trait_id = impl_trait_id_for_index(module, impl_signature, trait_defs);
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
                            name: binding.name,
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

/// Builds the associated-value index for one module.
pub fn collect_extension_associated_value_index_for_module(
    module: &ExtensionMethodIndexModuleInput<'_>,
    trait_defs: &HashSet<GlobalDefId>,
) -> (ExtensionAssociatedValues, Vec<Diagnostic>) {
    let mut values = ExtensionAssociatedValues::default();
    let mut diagnostics = Vec::new();
    for impl_signature in &module.signatures.trait_impls {
        let target_ty = module.normalization.normalize(impl_signature.target_ty);
        let trait_id = impl_trait_id_for_index(module, impl_signature, trait_defs);
        if !is_extendable_target(module.type_store, target_ty) {
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
                    name: associated_value.name,
                    def_id: GlobalDefId {
                        module_id: module.module_id,
                        def_id: associated_value.def_id,
                    },
                    impl_id: impl_signature.impl_id,
                    target_ty,
                    trait_id,
                    visibility: associated_value.visibility,
                },
                nominal_target_def_id(module.type_store, target_ty),
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
    let trait_ty = impl_signature.trait_ty?;
    let span = impl_signature.trait_span.unwrap_or(impl_signature.span);
    let ty = module.normalization.normalize(trait_ty);
    match module.type_store.get(ty).cloned() {
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

fn impl_trait_id_for_index(
    module: &ExtensionMethodIndexModuleInput<'_>,
    impl_signature: &TraitImplSignature,
    trait_defs: &HashSet<GlobalDefId>,
) -> Option<TraitId> {
    let trait_ty = impl_signature.trait_ty?;
    let ty = module.normalization.normalize(trait_ty);
    match module.type_store.get(ty).cloned() {
        Some(TyKind::Nominal { def_id, .. }) => trait_defs
            .contains(&def_id)
            .then_some(TraitId::Source(def_id)),
        Some(TyKind::BuiltinTrait { trait_id, .. }) => Some(TraitId::Builtin(trait_id)),
        _ => None,
    }
}

fn impl_trait_args_and_consts(
    module: &ExtensionModuleInput<'_>,
    impl_signature: &TraitImplSignature,
    expected_trait_id: Option<TraitId>,
) -> Option<(Vec<nia_ids::InternedTyId>, Vec<nia_ty::ConstGenericArg>)> {
    let ty = module.normalization.normalize(impl_signature.trait_ty?);
    match (expected_trait_id, module.type_store.get(ty)) {
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
    match (expected_trait_id, module.type_store.get(ty)) {
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
}

fn trait_signature_ref(
    trait_signatures: &HashMap<GlobalDefId, ProgramTraitSignature>,
    trait_id: GlobalDefId,
) -> Option<TraitSignatureRef<'_>> {
    trait_signatures
        .get(&trait_id)
        .map(|signature| TraitSignatureRef {
            signature: &signature.signature,
        })
}

fn validate_supertraits(
    module: &ExtensionModuleInput<'_>,
    input: ExtensionMethodValidationInput<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (trait_def_id, trait_signature) in &module.signatures.traits {
        for supertrait in &trait_signature.supertraits {
            let _ = supertrait_id(
                module,
                supertrait.ty,
                supertrait.span,
                input.trait_defs,
                diagnostics,
            );
        }
        validate_supertrait_associated_binding_conflicts(
            module,
            input.trait_signatures,
            input.symbols,
            GlobalDefId {
                module_id: module.module_id,
                def_id: *trait_def_id,
            },
            trait_signature,
            diagnostics,
        );
    }
}

fn validate_supertrait_associated_binding_conflicts(
    module: &ExtensionModuleInput<'_>,
    trait_signatures: &HashMap<GlobalDefId, ProgramTraitSignature>,
    symbols: &SymbolTable,
    trait_id: GlobalDefId,
    trait_signature: &TraitSignature,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let append = module.type_store.append_for_module(module.module_id);
    let mut trait_args = Vec::new();
    let mut trait_const_args = Vec::new();
    for parameter in &trait_signature.generic_params {
        match parameter.kind {
            GenericParamSignatureKind::Type => {
                trait_args.push(append.intern(TyKind::GenericParam(parameter.name)));
            }
            GenericParamSignatureKind::Const { ty } => {
                trait_const_args.push(nia_ty::ConstGenericArg {
                    ty,
                    value: nia_ty::ConstGenericValue::GenericParam(parameter.name),
                });
            }
        }
    }
    let mut assumptions = Vec::new();
    let mut bindings = Vec::new();
    push_trait_goal_assumption_with_supertraits(
        TraitGoalExpansionContext {
            type_store: module.type_store,
            module,
            trait_signatures,
        },
        TraitGoal {
            self_ty: append.intern(TyKind::SelfParam),
            trait_id: TraitId::Source(trait_id),
            trait_args,
            trait_const_args,
        },
        &mut assumptions,
        &mut bindings,
    );
    let mut checked = Vec::new();
    for binding in &bindings {
        let duplicate = checked
            .iter()
            .find(|existing: &&AssociatedTypeProjectionEq| {
                existing.goal.trait_id == binding.goal.trait_id
                    && existing.name == binding.name
                    && existing.goal.trait_args.len() == binding.goal.trait_args.len()
                    && existing.goal.trait_const_args.len() == binding.goal.trait_const_args.len()
                    && existing
                        .goal
                        .trait_args
                        .iter()
                        .zip(&binding.goal.trait_args)
                        .all(|(left, right)| {
                            types_equivalent(module.type_store, module.lowering, *left, *right)
                        })
                    && const_args_equivalent(
                        module.type_store,
                        module.lowering,
                        &existing.goal.trait_const_args,
                        &binding.goal.trait_const_args,
                    )
            });
        if let Some(existing) = duplicate {
            if !types_equivalent(module.type_store, module.lowering, existing.ty, binding.ty) {
                diagnostics.push(Diagnostic::user_error_at(
                    codes::NAME_RESOLUTION,
                    trait_signature.span,
                    format!(
                        "conflicting inherited associated type binding `{}`",
                        symbol_name(symbols, binding.name)
                    ),
                ));
            }
        } else {
            checked.push(binding.clone());
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
    match module.type_store.get(ty).cloned() {
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
    input: ExtensionMethodValidationInput<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let Some(trait_signature) = trait_signature_ref(input.trait_signatures, trait_id) else {
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
            let name = symbol_name(input.symbols, associated_type.name);
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
            let name = symbol_name(input.symbols, associated_value.name);
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                associated_value.span,
                format!("associated const `{name}` is not a member of implemented trait"),
            ));
            continue;
        };
        let Some(actual_ty) = module
            .signatures
            .consts
            .get(&associated_value.def_id)
            .and_then(|signature| signature.explicit_type)
        else {
            let name = symbol_name(input.symbols, associated_value.name);
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                associated_value.span,
                format!(
                    "associated const `{name}` requires an explicit type to satisfy the trait requirement"
                ),
            ));
            continue;
        };
        if !trait_associated_const_type_matches(TraitAssociatedConstTypeMatch {
            type_store: input.type_store,
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
            let name = symbol_name(input.symbols, associated_value.name);
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                associated_value.span,
                format!(
                    "implementation of associated const `{name}` does not match the trait requirement"
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
            let name = symbol_name(input.symbols, required.name);
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
            let name = symbol_name(input.symbols, required.name);
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                impl_signature.span,
                format!("missing definition for associated const `{name}`"),
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
            let name = symbol_name(input.symbols, method.name);
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                method.span,
                format!("method `{name}` is not a member of implemented trait"),
            ));
        }
    }
    let trait_goal = TraitGoal {
        self_ty: target_ty,
        trait_id: TraitId::Source(trait_id),
        trait_args: trait_args.clone(),
        trait_const_args: trait_const_args.clone(),
    };
    validate_supertrait_impls(
        module,
        impl_signature,
        trait_signature,
        &trait_goal,
        input,
        diagnostics,
    );
    let append = input.type_store.append_for_module(module.module_id);
    for required in &trait_signature.signature.methods {
        let Some(method) = impl_signature
            .methods
            .iter()
            .find(|method| method.name == required.name)
        else {
            if !required.has_default {
                let name = symbol_name(input.symbols, required.name);
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
        let Some(required_signature) = lower_trait_method_signature(TraitMethodSubstitution {
            append: &append,
            module,
            type_store: input.type_store,
            signature: &required.signature,
            trait_generic_params: &trait_signature.signature.generic_params,
            trait_args: &trait_args,
            trait_const_args: &trait_const_args,
            self_ty: target_ty,
            trait_id,
            impl_signature,
        }) else {
            diagnostics.push(Diagnostic::internal_error_at(
                codes::NAME_RESOLUTION,
                impl_signature.span,
                "trait method arguments do not match declaration parameters",
            ));
            continue;
        };
        let actual_signature = normalize_impl_method_signature(ImplMethodSignatureNormalize {
            append: &append,
            module,
            type_store: input.type_store,
            signature: actual,
            trait_args: &trait_args,
            trait_const_args: &trait_const_args,
            self_ty: target_ty,
            trait_id,
            impl_signature,
        });
        let goal_context = TraitGoalExpansionContext {
            type_store: input.type_store,
            module,
            trait_signatures: input.trait_signatures,
        };
        let validation_trait_impls = trait_impls_for_trait_goal_and_supertraits(
            goal_context,
            trait_goal.clone(),
            input.trait_impls_for_trait,
        );
        if !trait_method_signature_matches(TraitMethodSignatureMatch {
            type_store: input.type_store,
            module,
            trait_impls: &validation_trait_impls,
            trait_goal: trait_goal.clone(),
            impl_signature,
            trait_signatures: input.trait_signatures,
            required: &required_signature,
            actual: &actual_signature,
        }) {
            let name = symbol_name(input.symbols, required.name);
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

struct TraitAssociatedConstTypeMatch<'a> {
    type_store: &'a TypeStore,
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

fn trait_associated_const_type_matches(input: TraitAssociatedConstTypeMatch<'_>) -> bool {
    let append = input.type_store.append_for_module(input.module.module_id);
    let Some((substitutions, const_substitutions)) = substitutions_from_generic_params(
        &input.trait_signature.signature.generic_params,
        input.trait_args,
        input.trait_const_args,
    ) else {
        return false;
    };
    let projection_context = Some(ProjectionImplContext {
        trait_id: input.trait_id,
        trait_args: input.trait_args,
        trait_const_args: input.trait_const_args,
        self_ty: input.target_ty,
        associated_types: &input.impl_signature.associated_types,
    });
    let required = substitute_type(
        &append,
        input.module,
        input.type_store,
        input.required_ty,
        &substitutions,
        &const_substitutions,
        TypeSubstitutionTarget {
            projection: projection_context,
            self_ty: Some(input.target_ty),
        },
    );
    let actual = substitute_type(
        &append,
        input.module,
        input.type_store,
        input.actual_ty,
        &SymbolMap::default(),
        &SymbolMap::default(),
        TypeSubstitutionTarget {
            projection: projection_context,
            self_ty: None,
        },
    );
    types_equivalent_in_store(input.type_store, required, actual)
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
            module.type_store,
            target_ty,
            supertrait_id,
            &supertrait_args,
            &[],
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
        type_store: module.type_store,
        normalization: module.normalization,
        is_enum: |ty| match module.type_store.get(module.normalization.normalize(ty)) {
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
        || actual.return_type
            == module
                .type_store
                .append_for_module(module.module_id)
                .intern(TyKind::Error)
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
    let Some(TyKind::Pointer { is_readonly, elem }) = module.type_store.get(actual.return_type)
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
    types_equivalent(module.type_store, module.lowering, *elem, associated_type)
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
    if !types_equivalent(module.type_store, module.lowering, range_param.ty, range_ty) {
        return false;
    }
    let Some(output) =
        associated_type_ty(impl_signature, BuiltinAssociatedType::Output.symbol_id())
    else {
        return false;
    };
    types_equivalent(
        module.type_store,
        module.lowering,
        actual.return_type,
        output,
    )
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
    let Some(TyKind::Optional { elem }) = module.type_store.get(actual_return) else {
        return false;
    };
    types_equivalent(module.type_store, module.lowering, *elem, item)
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
    types_equivalent(module.type_store, module.lowering, actual.return_type, iter)
}

fn builtin_impl_trait_args(
    module: &ExtensionModuleInput<'_>,
    impl_signature: &TraitImplSignature,
    trait_id: BuiltinTrait,
) -> Option<Vec<nia_ids::InternedTyId>> {
    let ty = impl_signature.trait_ty?;
    let ty = module.normalization.normalize(ty);
    match module.type_store.get(ty) {
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
    trait_signature: TraitSignatureRef<'_>,
    trait_goal: &TraitGoal,
    input: ExtensionMethodValidationInput<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let append = input.type_store.append_for_module(module.module_id);
    for supertrait in &trait_signature.signature.supertraits {
        let Some(supertrait_ty) = substitute_trait_bound(
            &append,
            input.type_store,
            supertrait.ty,
            &trait_signature.signature.generic_params,
            &trait_goal.trait_args,
            &trait_goal.trait_const_args,
        ) else {
            diagnostics.push(Diagnostic::internal_error_at(
                codes::NAME_RESOLUTION,
                impl_signature.span,
                "trait instance arguments do not match declaration parameters",
            ));
            continue;
        };
        // Source traits may inherit builtin traits (for example `DerefMut`'s
        // `Deref` relationship). Keep the same explicit-impl check for both
        // forms; skipping non-nominal bounds would make the inherited
        // capability appear available without a witness.
        let Some((supertrait_id, supertrait_args, supertrait_const_args)) =
            trait_id_and_args(input.type_store, supertrait_ty)
        else {
            continue;
        };
        let trait_impls = (input.trait_impls_for_trait)(supertrait_id);
        let supertrait_goal = TraitGoal {
            self_ty: trait_goal.self_ty,
            trait_id: supertrait_id,
            trait_args: supertrait_args.clone(),
            trait_const_args: supertrait_const_args.clone(),
        };
        let has_intrinsic_witness = matches!(supertrait_id, TraitId::Builtin(BuiltinTrait::Sized))
            && intrinsically_sized_target(
                input.type_store,
                module.normalization,
                trait_goal.self_ty,
            );
        if !has_intrinsic_witness
            && !has_matching_trait_impl(
                input.type_store,
                trait_goal.self_ty,
                supertrait_id,
                &supertrait_args,
                &supertrait_const_args,
                &trait_impls,
            )
        {
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                impl_signature.span,
                format!(
                    "implementation of trait requires explicit implementation of supertrait `{}`",
                    match supertrait_id {
                        TraitId::Source(supertrait_def_id) => {
                            trait_name(module, supertrait_def_id, input.symbols).to_string()
                        }
                        TraitId::Builtin(supertrait_id) => supertrait_id.name().to_string(),
                    }
                ),
            ));
            continue;
        }
        let Some((substitutions, const_substitutions)) = substitutions_from_generic_params(
            &trait_signature.signature.generic_params,
            &trait_goal.trait_args,
            &trait_goal.trait_const_args,
        ) else {
            diagnostics.push(Diagnostic::internal_error_at(
                codes::NAME_RESOLUTION,
                impl_signature.span,
                "supertrait arguments do not match declaration parameters",
            ));
            continue;
        };
        let bindings = supertrait
            .associated_type_bindings
            .iter()
            .map(|binding| {
                (
                    binding.name,
                    substitute_type(
                        &append,
                        module,
                        input.type_store,
                        binding.ty,
                        &substitutions,
                        &const_substitutions,
                        TypeSubstitutionTarget {
                            projection: None,
                            self_ty: Some(trait_goal.self_ty),
                        },
                    ),
                )
            })
            .collect::<Vec<_>>();
        if !supertrait_associated_bindings_hold(
            module,
            input.type_store,
            &trait_impls,
            &supertrait_goal,
            &bindings,
        ) {
            diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                impl_signature.span,
                format!(
                    "implementation of trait does not satisfy associated type bindings of supertrait `{}`",
                    match supertrait_id {
                        TraitId::Source(supertrait_def_id) => {
                            trait_name(module, supertrait_def_id, input.symbols).to_string()
                        }
                        TraitId::Builtin(supertrait_id) => supertrait_id.name().to_string(),
                    }
                ),
            ));
        }
    }
}

fn supertrait_associated_bindings_hold(
    module: &ExtensionModuleInput<'_>,
    type_store: &TypeStore,
    trait_impls: &[ProgramTraitImplSignature],
    goal: &TraitGoal,
    bindings: &[(SymbolId, InternedTyId)],
) -> bool {
    if bindings.is_empty() {
        return true;
    }
    let context = TraitSolverContext {
        type_store,
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
    let mut solver = context.solver(&[]);
    bindings.iter().all(|(name, expected_ty)| {
        solver
            .resolve_associated_type(
                goal.self_ty,
                goal.trait_id,
                &goal.trait_args,
                &goal.trait_const_args,
                name,
            )
            .is_some_and(|actual_ty| solver.types_equivalent(actual_ty, *expected_ty))
    })
}

fn substitute_trait_bound(
    append: &TypeStoreAppend,
    type_store: &TypeStore,
    ty: nia_ids::InternedTyId,
    trait_generic_params: &[GenericParamSignature],
    trait_args: &[nia_ids::InternedTyId],
    trait_const_args: &[nia_ty::ConstGenericArg],
) -> Option<nia_ids::InternedTyId> {
    let (substitutions, const_substitutions) =
        generic_argument_substitutions(trait_generic_params, trait_args, trait_const_args)?;
    Some(nia_ty::substitute_ty(
        type_store,
        append,
        ty,
        &|name| substitutions.get(name).copied(),
        &|name| const_substitutions.get(name).cloned(),
        None,
    ))
}

fn has_matching_trait_impl(
    interner: &TypeStore,
    target_ty: nia_ids::InternedTyId,
    trait_id: TraitId,
    trait_args: &[nia_ids::InternedTyId],
    trait_const_args: &[nia_ty::ConstGenericArg],
    trait_impls: &[ProgramTraitImplSignature],
) -> bool {
    trait_impls.iter().any(|impl_signature| {
        if impl_signature.trait_id != trait_id {
            return false;
        }
        types_equivalent_in_store(interner, impl_signature.target_ty, target_ty)
            && impl_signature.trait_args.len() == trait_args.len()
            && impl_signature.trait_const_args.len() == trait_const_args.len()
            && impl_signature
                .trait_args
                .iter()
                .zip(trait_args)
                .all(|(left, right)| types_equivalent_in_store(interner, *left, *right))
            && const_args_equivalent_in_store(
                interner,
                &impl_signature.trait_const_args,
                trait_const_args,
            )
    })
}

fn intrinsically_sized_target(
    type_store: &TypeStore,
    normalization: &TypeNormalization,
    ty: InternedTyId,
) -> bool {
    match type_store.get(normalization.normalize(ty)) {
        Some(
            TyKind::Error
            | TyKind::ConstOnly
            | TyKind::Opaque
            | TyKind::GenericParam(_)
            | TyKind::SelfParam
            | TyKind::SlicePointee { .. }
            | TyKind::TraitObjectPointee { .. }
            | TyKind::CallablePointee { .. },
        )
        | None => false,
        Some(_) => true,
    }
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
        .unwrap_or_else(|| {
            format!(
                "trait#{}.{}",
                trait_id.module_id.local_index(),
                trait_id.def_id.0
            )
        })
}

struct TraitMethodSignatureMatch<'a> {
    type_store: &'a TypeStore,
    module: &'a ExtensionModuleInput<'a>,
    trait_impls: &'a [ProgramTraitImplSignature],
    trait_goal: TraitGoal,
    impl_signature: &'a TraitImplSignature,
    trait_signatures: &'a HashMap<GlobalDefId, ProgramTraitSignature>,
    required: &'a nia_item_signatures::FunctionSignature,
    actual: &'a nia_item_signatures::FunctionSignature,
}

fn trait_method_signature_matches(input: TraitMethodSignatureMatch<'_>) -> bool {
    let TraitGoal {
        self_ty,
        trait_id,
        trait_args,
        trait_const_args,
    } = &input.trait_goal;
    let mut assumptions = Vec::new();
    let mut associated_type_assumptions = Vec::new();
    push_trait_goal_assumption_with_supertraits(
        TraitGoalExpansionContext {
            type_store: input.type_store,
            module: input.module,
            trait_signatures: input.trait_signatures,
        },
        input.trait_goal.clone(),
        &mut assumptions,
        &mut associated_type_assumptions,
    );
    associated_type_assumptions.extend(
        input
            .impl_signature
            .associated_types
            .iter()
            .map(|associated_type| AssociatedTypeProjectionEq {
                goal: TraitGoal {
                    self_ty: *self_ty,
                    trait_id: *trait_id,
                    trait_args: trait_args.clone(),
                    trait_const_args: trait_const_args.clone(),
                },
                name: associated_type.name,
                ty: input.module.normalization.normalize(associated_type.ty),
            })
            .collect::<Vec<_>>(),
    );
    push_where_predicate_solver_assumptions(
        input.module,
        input.type_store,
        &input.impl_signature.where_predicates,
        input.trait_signatures,
        &mut assumptions,
        &mut associated_type_assumptions,
    );
    let const_expr_value = |id, _ty| {
        input
            .module
            .lowering
            .const_expr_summaries
            .get(&id)
            .and_then(|summary| summary.literal_array_len)
            .map(|value| {
                nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(u128::from(value)))
            })
    };
    let context = TraitSolverContext {
        type_store: input.type_store,
        normalization: input.module.normalization,
        trait_impls: input.trait_impls,
        trait_impl_index: None,
        layouts: None,
        local_module_id: input.module.module_id,
        local_enums: &input.module.signatures.enums,
        program_is_enum: None,
        const_expr_value: Some(&const_expr_value),
        impl_is_visible: None,
    };
    let mut solver =
        context.solver_with_associated_type_assumptions(&assumptions, &associated_type_assumptions);
    input.required.generics == input.actual.generics
        && input.required.where_predicates == input.actual.where_predicates
        && input.required.params.len() == input.actual.params.len()
        && input
            .required
            .params
            .iter()
            .zip(input.actual.params.iter())
            .all(|(required, actual)| {
                required.receiver == actual.receiver
                    && solver.types_equivalent(required.ty, actual.ty)
            })
        && solver.types_equivalent(input.required.return_type, input.actual.return_type)
        && input.required.is_variadic == input.actual.is_variadic
}

#[derive(Clone, Copy)]
struct TraitGoalExpansionContext<'a> {
    type_store: &'a TypeStore,
    module: &'a ExtensionModuleInput<'a>,
    trait_signatures: &'a HashMap<GlobalDefId, ProgramTraitSignature>,
}

fn trait_impls_for_trait_goal_and_supertraits(
    context: TraitGoalExpansionContext<'_>,
    goal: TraitGoal,
    trait_impls_for_trait: &dyn Fn(TraitId) -> Vec<ProgramTraitImplSignature>,
) -> Vec<ProgramTraitImplSignature> {
    let mut goals = Vec::new();
    let mut associated_type_assumptions = Vec::new();
    push_trait_goal_assumption_with_supertraits(
        context,
        goal,
        &mut goals,
        &mut associated_type_assumptions,
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
    type_store: &TypeStore,
    predicates: &[WherePredicateSignature],
    trait_signatures: &HashMap<GlobalDefId, ProgramTraitSignature>,
    assumptions: &mut Vec<TraitGoal>,
    associated_type_assumptions: &mut Vec<AssociatedTypeProjectionEq>,
) {
    for predicate in predicates {
        let self_ty = module.normalization.normalize(predicate.ty);
        for bound in &predicate.bounds {
            let trait_ty = module.normalization.normalize(bound.trait_ty);
            let Some((trait_id, trait_args, trait_const_args)) =
                trait_id_and_args(type_store, trait_ty)
            else {
                continue;
            };
            push_trait_goal_assumption_with_supertraits(
                TraitGoalExpansionContext {
                    type_store,
                    module,
                    trait_signatures,
                },
                TraitGoal {
                    self_ty,
                    trait_id,
                    trait_args: trait_args.clone(),
                    trait_const_args: trait_const_args.clone(),
                },
                assumptions,
                associated_type_assumptions,
            );
            for binding in &bound.associated_type_bindings {
                let ty = module.normalization.normalize(binding.ty);
                associated_type_assumptions.push(AssociatedTypeProjectionEq {
                    goal: TraitGoal {
                        self_ty,
                        trait_id,
                        trait_args: trait_args.clone(),
                        trait_const_args: trait_const_args.clone(),
                    },
                    name: binding.name,
                    ty,
                });
            }
        }
    }
}

fn push_trait_goal_assumption_with_supertraits(
    context: TraitGoalExpansionContext<'_>,
    goal: TraitGoal,
    assumptions: &mut Vec<TraitGoal>,
    associated_type_assumptions: &mut Vec<AssociatedTypeProjectionEq>,
) {
    push_trait_goal_assumption_with_supertraits_inner(
        context,
        goal,
        assumptions,
        associated_type_assumptions,
        &mut Vec::new(),
    );
}

fn push_trait_goal_assumption_with_supertraits_inner(
    context: TraitGoalExpansionContext<'_>,
    goal: TraitGoal,
    assumptions: &mut Vec<TraitGoal>,
    associated_type_assumptions: &mut Vec<AssociatedTypeProjectionEq>,
    visited: &mut Vec<TraitGoal>,
) {
    if visited.iter().any(|existing| {
        trait_goals_equivalent(context.type_store, context.module.lowering, existing, &goal)
    }) {
        return;
    }
    // This guard is path-local: sibling supertraits must still be expanded after one
    // recursive or unavailable branch returns.
    visited.push(goal.clone());
    if !assumptions.iter().any(|assumption| {
        trait_goals_equivalent(
            context.type_store,
            context.module.lowering,
            assumption,
            &goal,
        )
    }) {
        assumptions.push(goal.clone());
    }
    match goal.trait_id {
        TraitId::Builtin(trait_id) => {
            for supertrait in trait_id.supertraits() {
                let supertrait_args = if supertrait.preserves_trait_args {
                    goal.trait_args.clone()
                } else {
                    Vec::new()
                };
                push_trait_goal_assumption_with_supertraits_inner(
                    context,
                    TraitGoal {
                        self_ty: goal.self_ty,
                        trait_id: TraitId::Builtin(supertrait.trait_id),
                        trait_args: supertrait_args,
                        trait_const_args: Vec::new(),
                    },
                    assumptions,
                    associated_type_assumptions,
                    visited,
                );
            }
        }
        TraitId::Source(trait_id) => {
            let Some(trait_signature) = trait_signature_ref(context.trait_signatures, trait_id)
            else {
                visited.pop();
                return;
            };
            let Some((substitutions, const_substitutions)) = substitutions_from_generic_params(
                &trait_signature.signature.generic_params,
                &goal.trait_args,
                &goal.trait_const_args,
            ) else {
                visited.pop();
                return;
            };
            let append = context
                .type_store
                .append_for_module(context.module.module_id);
            for supertrait in &trait_signature.signature.supertraits {
                let supertrait_ty = substitute_type(
                    &append,
                    context.module,
                    context.type_store,
                    supertrait.ty,
                    &substitutions,
                    &const_substitutions,
                    TypeSubstitutionTarget {
                        projection: None,
                        self_ty: Some(goal.self_ty),
                    },
                );
                let Some((supertrait_id, supertrait_args, supertrait_const_args)) =
                    trait_id_and_args(context.type_store, supertrait_ty)
                else {
                    continue;
                };
                let supertrait_goal = TraitGoal {
                    self_ty: goal.self_ty,
                    trait_id: supertrait_id,
                    trait_args: supertrait_args,
                    trait_const_args: supertrait_const_args,
                };
                for binding in &supertrait.associated_type_bindings {
                    associated_type_assumptions.push(AssociatedTypeProjectionEq {
                        goal: supertrait_goal.clone(),
                        name: binding.name,
                        ty: substitute_type(
                            &append,
                            context.module,
                            context.type_store,
                            binding.ty,
                            &substitutions,
                            &const_substitutions,
                            TypeSubstitutionTarget {
                                projection: None,
                                self_ty: Some(goal.self_ty),
                            },
                        ),
                    });
                }
                push_trait_goal_assumption_with_supertraits_inner(
                    context,
                    supertrait_goal,
                    assumptions,
                    associated_type_assumptions,
                    visited,
                );
            }
        }
    }
    visited.pop();
}

fn trait_goals_equivalent(
    type_store: &TypeStore,
    lowering: &TypeLowering,
    left: &TraitGoal,
    right: &TraitGoal,
) -> bool {
    left.trait_id == right.trait_id
        && types_equivalent(type_store, lowering, left.self_ty, right.self_ty)
        && left.trait_args.len() == right.trait_args.len()
        && left
            .trait_args
            .iter()
            .zip(&right.trait_args)
            .all(|(left, right)| types_equivalent(type_store, lowering, *left, *right))
        && left.trait_const_args.len() == right.trait_const_args.len()
        && left
            .trait_const_args
            .iter()
            .zip(&right.trait_const_args)
            .all(|(left, right)| {
                const_args_equivalent(
                    type_store,
                    lowering,
                    std::slice::from_ref(left),
                    std::slice::from_ref(right),
                )
            })
}

fn is_extendable_target(interner: &TypeStore, ty: nia_ids::InternedTyId) -> bool {
    match interner.get(ty) {
        Some(TyKind::Error | TyKind::ConstOnly | TyKind::Opaque) | None => false,
        Some(TyKind::Primitive(PrimitiveTy::Never)) => false,
        Some(TyKind::Array { len, .. }) => !matches!(len, nia_ty::ArrayLenTy::Infer),
        Some(
            TyKind::Primitive(_)
            | TyKind::Tuple(_)
            | TyKind::Vector { .. }
            | TyKind::Pointer { .. }
            | TyKind::VolatilePointer { .. }
            | TyKind::Slice { .. }
            | TyKind::FunctionPointer { .. }
            | TyKind::Callable { .. }
            | TyKind::CallablePointee { .. }
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
            | TyKind::ClosureState { .. }
            | TyKind::GenericParam(_)
            | TyKind::SelfParam,
        ) => true,
    }
}

fn nominal_target_def_id(interner: &TypeStore, ty: InternedTyId) -> Option<GlobalDefId> {
    match interner.get(ty) {
        Some(TyKind::Nominal { def_id, .. }) => Some(*def_id),
        _ => None,
    }
}
