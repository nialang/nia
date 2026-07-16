// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use nia_defs::{
    DefCollection, ExtensionAssociatedValues, ExtensionMethods, PublicNamespace,
    PublicSurfaceLookup, VisibleExtensionAssociatedValue, VisibleExtensionMethod,
    VisibleExtensionMethods,
};
use nia_ids::{GlobalDefId, TraitId, Visibility};
use nia_item_signatures::{ProgramTraitImplSignature, ProgramTypeAliasSignature};
use nia_ty::TyKind;
use nia_type_normalize::TypeNormalization;

pub type TypeNormalizationResolver<'a> = &'a dyn Fn(nia_ids::ModuleId) -> Option<TypeNormalization>;
pub type NominalExtensionProviderResolver<'a> =
    &'a dyn Fn(&[GlobalDefId]) -> Vec<nia_ids::ModuleId>;

#[derive(Debug, Clone, PartialEq)]
pub struct VisibleExtensionsForModule {
    pub methods: VisibleExtensionMethods,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VisibleTraitImplsForModule {
    pub trait_impls: Vec<ProgramTraitImplSignature>,
    pub trait_impl_index: nia_item_signatures::ProgramTraitImplIndex,
}

pub trait ProgramDefsResolver {
    fn defs(&self, module_id: nia_ids::ModuleId) -> Option<Arc<DefCollection>>;
}

pub trait ProgramUsingScopeResolver {
    fn using_scope(&self, module_id: nia_ids::ModuleId) -> Option<Arc<nia_defs::ModuleUsingScope>>;
}

impl<F> ProgramUsingScopeResolver for F
where
    F: Fn(nia_ids::ModuleId) -> Option<Arc<nia_defs::ModuleUsingScope>>,
{
    fn using_scope(&self, module_id: nia_ids::ModuleId) -> Option<Arc<nia_defs::ModuleUsingScope>> {
        self(module_id)
    }
}

pub struct VisibleExtensionsInput<'a> {
    pub module_id: nia_ids::ModuleId,
    pub graph: &'a dyn nia_imports::ModuleGraphLookup,
    pub using_scope: &'a nia_defs::ModuleUsingScope,
    pub using_scopes: &'a dyn ProgramUsingScopeResolver,
    pub public_surfaces: &'a dyn PublicSurfaceLookup,
    pub defs: &'a dyn ProgramDefsResolver,
    pub normalizations: TypeNormalizationResolver<'a>,
    pub visible_type_signatures: VisibleTypeSignatures<'a>,
    pub extensions: &'a ExtensionMethods,
    pub associated_values: &'a ExtensionAssociatedValues,
    pub trait_impls: &'a [ProgramTraitImplSignature],
    pub nominal_extension_providers: NominalExtensionProviderResolver<'a>,
    pub visible_modules: Option<&'a [nia_ids::ModuleId]>,
}

#[derive(Clone, Copy)]
pub struct VisibleTypeSignatures<'a> {
    pub type_alias: &'a dyn Fn(GlobalDefId) -> Option<ProgramTypeAliasSignature>,
}

struct VisibleExtensionResolverCache<'a> {
    defs: &'a dyn ProgramDefsResolver,
    normalizations: TypeNormalizationResolver<'a>,
    normalization_cache: HashMap<nia_ids::ModuleId, Option<TypeNormalization>>,
}

impl<'a> VisibleExtensionResolverCache<'a> {
    fn new(
        defs: &'a dyn ProgramDefsResolver,
        normalizations: TypeNormalizationResolver<'a>,
    ) -> Self {
        Self {
            defs,
            normalizations,
            normalization_cache: HashMap::new(),
        }
    }

    fn defs(&self, module_id: nia_ids::ModuleId) -> Option<Arc<DefCollection>> {
        self.defs.defs(module_id)
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

pub fn visible_extensions_for_module(
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
        trait_impls: _,
        nominal_extension_providers,
        visible_modules,
    } = input;
    let mut resolver_cache = VisibleExtensionResolverCache::new(defs, normalizations);
    let computed_visible_modules;
    let visible_modules = if let Some(visible_modules) = visible_modules {
        visible_modules
    } else {
        let visibility_context = VisibilityClosureContext {
            module_id,
            graph,
            using_scope,
            using_scopes,
            defs,
            normalizations,
            visible_type_signatures,
            nominal_extension_providers,
        };
        computed_visible_modules = declared_module_closure(&visibility_context);
        &computed_visible_modules
    };
    let witness_modules = visible_modules;
    let imported_visible_modules = visible_modules
        .iter()
        .copied()
        .filter(|visible_module| *visible_module != module_id)
        .collect::<Vec<_>>();
    let mut visible = VisibleExtensionMethods::default();
    let extension_visibility_allows = |visibility, defining_module| {
        nia_imports::visibility_allows(visibility, graph, defining_module, module_id)
    };
    extensions.for_each_visible_method(
        module_id,
        imported_visible_modules.iter().copied(),
        extension_visibility_allows,
        |method| {
            if resolver_cache
                .defs(method.def_id.module_id)
                .is_none_or(|defs| defs.defs.get(method.def_id.def_id).is_none())
            {
                return;
            }
            let trait_is_visible = method.trait_id.is_some_and(|trait_id| {
                witness_modules.contains(&method.def_id.module_id)
                    && trait_id_is_visible(
                        module_id,
                        witness_modules,
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
            visible.insert(
                method.impl_id,
                target_ty,
                VisibleExtensionMethod {
                    name: method.name,
                    def_id: method.def_id,
                    impl_id: method.impl_id,
                    effective_generics: method.effective_generics.clone(),
                    trait_id: method.trait_id,
                    trait_args: method
                        .trait_args
                        .iter()
                        .map(|arg| method_normalization.normalize(*arg))
                        .collect(),
                    where_predicates: method.where_predicates.clone(),
                    is_callable: extension_visibility_allows(
                        method.visibility,
                        method.def_id.module_id,
                    ),
                    is_trait_witness: trait_is_visible,
                },
            );
        },
    );
    associated_values.for_each_visible_value(
        module_id,
        imported_visible_modules.iter().copied(),
        extension_visibility_allows,
        |value| {
            if resolver_cache
                .defs(value.def_id.module_id)
                .is_none_or(|defs| defs.defs.get(value.def_id.def_id).is_none())
            {
                return;
            }
            let Some(value_normalization) = resolver_cache.normalization(value.def_id.module_id)
            else {
                return;
            };
            let target_ty = value_normalization.normalize(value.target_ty);
            visible.insert_associated_value(
                value.impl_id,
                target_ty,
                VisibleExtensionAssociatedValue {
                    name: value.name,
                    def_id: value.def_id,
                },
            );
            visible.insert_trait_witness_impl(value.def_id.module_id, value.impl_id);
        },
    );
    VisibleExtensionsForModule { methods: visible }
}

pub fn visible_trait_impls_for_module(
    input: VisibleExtensionsInput<'_>,
) -> VisibleTraitImplsForModule {
    let VisibleExtensionsInput {
        module_id,
        graph,
        using_scope,
        using_scopes,
        public_surfaces,
        defs,
        normalizations,
        visible_type_signatures,
        trait_impls,
        nominal_extension_providers,
        visible_modules,
        ..
    } = input;
    let mut resolver_cache = VisibleExtensionResolverCache::new(defs, normalizations);
    let computed_visible_modules;
    let visible_modules = if let Some(visible_modules) = visible_modules {
        visible_modules
    } else {
        let visibility_context = VisibilityClosureContext {
            module_id,
            graph,
            using_scope,
            using_scopes,
            defs,
            normalizations,
            visible_type_signatures,
            nominal_extension_providers,
        };
        computed_visible_modules = declared_module_closure(&visibility_context);
        &computed_visible_modules
    };
    let witness_modules = visible_modules;
    let trait_impls = trait_impls
        .iter()
        .filter(|impl_signature| {
            witness_modules.contains(&impl_signature.module_id)
                && trait_id_is_visible(
                    module_id,
                    witness_modules,
                    impl_signature.trait_id,
                    public_surfaces,
                    &mut resolver_cache,
                )
        })
        .cloned()
        .collect::<Vec<_>>();
    let trait_impl_index = nia_item_signatures::ProgramTraitImplIndex::new(&trait_impls);
    VisibleTraitImplsForModule {
        trait_impls,
        trait_impl_index,
    }
}

pub struct VisibleExtensionProviderModulesInput<'a> {
    pub module_id: nia_ids::ModuleId,
    pub graph: &'a dyn nia_imports::ModuleGraphLookup,
    pub using_scope: &'a nia_defs::ModuleUsingScope,
    pub using_scopes: &'a dyn ProgramUsingScopeResolver,
    pub defs: &'a dyn ProgramDefsResolver,
    pub normalizations: TypeNormalizationResolver<'a>,
    pub visible_type_signatures: VisibleTypeSignatures<'a>,
    pub nominal_extension_providers: NominalExtensionProviderResolver<'a>,
}

pub fn visible_extension_provider_modules(
    input: VisibleExtensionProviderModulesInput<'_>,
) -> Vec<nia_ids::ModuleId> {
    let context = visibility_closure_context(input);
    declared_module_closure(&context)
}

pub fn visible_trait_impl_modules(
    input: VisibleExtensionProviderModulesInput<'_>,
) -> Vec<nia_ids::ModuleId> {
    let context = visibility_closure_context(input);
    declared_module_closure_including_item_target_modules(&context)
}

fn visibility_closure_context<'a>(
    input: VisibleExtensionProviderModulesInput<'a>,
) -> VisibilityClosureContext<'a> {
    VisibilityClosureContext {
        module_id: input.module_id,
        graph: input.graph,
        using_scope: input.using_scope,
        using_scopes: input.using_scopes,
        defs: input.defs,
        normalizations: input.normalizations,
        visible_type_signatures: input.visible_type_signatures,
        nominal_extension_providers: input.nominal_extension_providers,
    }
}

fn trait_id_is_visible(
    current_module: nia_ids::ModuleId,
    imported_modules: &[nia_ids::ModuleId],
    trait_id: TraitId,
    public_surfaces: &dyn PublicSurfaceLookup,
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
    public_surfaces: &dyn PublicSurfaceLookup,
    module_id: nia_ids::ModuleId,
    def_id: GlobalDefId,
) -> bool {
    public_surfaces
        .public_surface(module_id)
        .is_some_and(|surface| {
            surface.types.values().any(|item| {
                item.target_module == def_id.module_id
                    && item.target_def_id == def_id.def_id
                    && item.namespace == PublicNamespace::Type
            })
        })
}

fn declared_module_closure(context: &VisibilityClosureContext<'_>) -> Vec<nia_ids::ModuleId> {
    declared_module_closure_inner(context, false)
}

fn declared_module_closure_including_item_target_modules(
    context: &VisibilityClosureContext<'_>,
) -> Vec<nia_ids::ModuleId> {
    declared_module_closure_inner(context, true)
}

fn declared_module_closure_inner(
    context: &VisibilityClosureContext<'_>,
    include_item_target_modules: bool,
) -> Vec<nia_ids::ModuleId> {
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    let mut pending_provider_targets = Vec::new();
    let mut resolved_provider_targets = HashSet::new();
    enqueue_using_scope_modules(context.using_scope, &mut queue);
    if include_item_target_modules {
        enqueue_using_scope_item_target_modules(context.using_scope, &mut queue);
    }
    collect_public_inherent_extension_provider_targets_for_using_scope(
        context,
        context.using_scope,
        &mut pending_provider_targets,
    );

    loop {
        while let Some(visible) = queue.pop_front() {
            if visible == context.module_id || !seen.insert(visible) {
                continue;
            }
            if let Some(using_scope) = context.using_scopes.using_scope(visible) {
                enqueue_using_scope_modules(&using_scope, &mut queue);
                if include_item_target_modules {
                    enqueue_using_scope_item_target_modules(&using_scope, &mut queue);
                }
                collect_public_inherent_extension_provider_targets_for_using_scope(
                    context,
                    &using_scope,
                    &mut pending_provider_targets,
                );
            }
        }

        pending_provider_targets.sort();
        pending_provider_targets.dedup();
        pending_provider_targets.retain(|target| resolved_provider_targets.insert(*target));
        if pending_provider_targets.is_empty() {
            break;
        }

        enqueue_public_inherent_extension_provider_modules_for_targets(
            context,
            &pending_provider_targets,
            &mut queue,
        );
        pending_provider_targets.clear();
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
}

fn enqueue_using_scope_item_target_modules(
    using_scope: &nia_defs::ModuleUsingScope,
    queue: &mut VecDeque<nia_ids::ModuleId>,
) {
    queue.extend(
        using_scope
            .values
            .values()
            .chain(using_scope.types.values())
            .map(|entry| entry.target_module),
    );
}

struct VisibilityClosureContext<'a> {
    module_id: nia_ids::ModuleId,
    graph: &'a dyn nia_imports::ModuleGraphLookup,
    using_scope: &'a nia_defs::ModuleUsingScope,
    using_scopes: &'a dyn ProgramUsingScopeResolver,
    defs: &'a dyn ProgramDefsResolver,
    normalizations: TypeNormalizationResolver<'a>,
    visible_type_signatures: VisibleTypeSignatures<'a>,
    nominal_extension_providers: NominalExtensionProviderResolver<'a>,
}

fn collect_public_inherent_extension_provider_targets_for_using_scope(
    context: &VisibilityClosureContext<'_>,
    using_scope: &nia_defs::ModuleUsingScope,
    targets: &mut Vec<GlobalDefId>,
) {
    targets.extend(
        using_scope
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
            }),
    );
}

fn enqueue_public_inherent_extension_provider_modules_for_targets(
    context: &VisibilityClosureContext<'_>,
    type_def_ids: &[GlobalDefId],
    queue: &mut VecDeque<nia_ids::ModuleId>,
) {
    if type_def_ids.is_empty() {
        return;
    }
    queue.extend(
        (context.nominal_extension_providers)(type_def_ids)
            .into_iter()
            .filter(|provider| {
                nia_imports::visibility_allows(
                    Visibility::Public,
                    context.graph,
                    *provider,
                    context.module_id,
                )
            }),
    );
}

fn nominal_def_id_for_public_type(
    def_id: GlobalDefId,
    defs: &dyn ProgramDefsResolver,
    normalizations: TypeNormalizationResolver<'_>,
    visible_type_signatures: VisibleTypeSignatures<'_>,
) -> Option<GlobalDefId> {
    let defs = defs.defs(def_id.module_id)?;
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
    let alias = (visible_type_signatures.type_alias)(def_id)?;
    let normalized = normalization.normalize(alias.signature.target);
    match normalization.interner.get(normalized) {
        Some(TyKind::Nominal { def_id, .. }) => Some(*def_id),
        _ => None,
    }
}
