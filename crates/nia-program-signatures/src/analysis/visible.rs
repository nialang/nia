// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use nia_defs::{
    DefCollection, ExtensionAssociatedValue, ExtensionMethod, VisibleExtensionAssociatedValue,
    VisibleExtensionMethod, VisibleExtensionMethods,
};
use nia_item_signatures::ProgramTraitImplSignature;
use nia_ty::TypeStore;
use nia_type_normalize::TypeNormalization;

/// Resolves a module id to its canonical type normalization context.
pub type TypeNormalizationResolver<'a> =
    &'a dyn Fn(nia_ids::ModuleId) -> Option<Arc<TypeNormalization>>;
#[derive(Debug, Clone, PartialEq)]
/// Extensions visible from one module after using-scope and visibility closure.
pub struct VisibleExtensionsForModule {
    /// Visible methods and associated values grouped by target.
    pub methods: Arc<VisibleExtensionMethods>,
}

#[derive(Debug, Clone, PartialEq)]
/// Trait implementations visible from one module, with a candidate index.
pub struct VisibleTraitImplsForModule {
    /// Visible program trait implementations.
    pub trait_impls: Vec<ProgramTraitImplSignature>,
    /// Index over the visible implementations.
    pub trait_impl_index: nia_item_signatures::ProgramTraitImplIndex,
}

/// Resolves module-local definition collections.
pub trait ProgramDefsResolver {
    /// Returns definitions for a module, if its semantic facts are available.
    fn defs(&self, module_id: nia_ids::ModuleId) -> Option<Arc<DefCollection>>;
}

/// Resolves the public module namespace of an imported module.
pub trait ProgramPublicSurfaceResolver {
    /// Returns the public surface for a module, if available.
    fn public_surface(
        &self,
        module_id: nia_ids::ModuleId,
    ) -> Option<Arc<nia_defs::ModulePublicSurface>>;
}

impl<F> ProgramPublicSurfaceResolver for F
where
    F: Fn(nia_ids::ModuleId) -> Option<Arc<nia_defs::ModulePublicSurface>>,
{
    fn public_surface(
        &self,
        module_id: nia_ids::ModuleId,
    ) -> Option<Arc<nia_defs::ModulePublicSurface>> {
        self(module_id)
    }
}

/// Inputs for visibility-aware extension and trait-implementation collection.
pub struct VisibleExtensionsInput<'a> {
    /// Module from which visibility is queried.
    pub module_id: nia_ids::ModuleId,
    /// Module graph used for visibility checks.
    pub graph: &'a dyn nia_imports::ModuleGraphLookup,
    /// Definition resolver for extension members and traits.
    pub defs: &'a dyn ProgramDefsResolver,
    /// Per-module canonical type-normalization resolver.
    pub normalizations: TypeNormalizationResolver<'a>,
    /// The sorted closure of modules whose inherent extensions are in scope.
    pub visible_modules: &'a [nia_ids::ModuleId],
    /// Modules whose trait implementations are facts here; see
    /// [`visible_trait_impl_modules`]. The slice must be sorted.
    pub trait_witness_modules: &'a [nia_ids::ModuleId],
}

struct VisibleExtensionResolverCache<'a> {
    defs: &'a dyn ProgramDefsResolver,
    normalizations: TypeNormalizationResolver<'a>,
    normalization_cache: HashMap<nia_ids::ModuleId, Option<Arc<TypeNormalization>>>,
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
            .and_then(|normalization| normalization.as_deref())
    }
}

/// Computes callable and witness extensions visible from one module.
pub fn visible_extensions_for_module<'a>(
    input: VisibleExtensionsInput<'_>,
    methods: impl IntoIterator<Item = &'a ExtensionMethod>,
    values: impl IntoIterator<Item = &'a ExtensionAssociatedValue>,
) -> VisibleExtensionsForModule {
    let VisibleExtensionsInput {
        module_id,
        graph,
        defs,
        normalizations,
        visible_modules,
        trait_witness_modules,
    } = input;
    let mut resolver_cache = VisibleExtensionResolverCache::new(defs, normalizations);
    let mut visible = VisibleExtensionMethods::default();
    let extension_visibility_allows = |visibility, defining_module| {
        nia_imports::visibility_allows(visibility, graph, defining_module, module_id)
    };
    for method in methods {
        let owner = method.def_id.module_id;
        if resolver_cache
            .defs(owner)
            .is_none_or(|defs| defs.defs.get(method.def_id.def_id).is_none())
        {
            continue;
        }
        let trait_is_visible = method.trait_id.is_some_and(|trait_id| {
            trait_witness_modules.binary_search(&owner).is_ok()
                && trait_declaration_allows(defs, graph, trait_id, module_id)
        });
        let Some(method_normalization) = resolver_cache.normalization(owner) else {
            continue;
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
                effective_const_generics: method.effective_const_generics.clone(),
                trait_id: method.trait_id,
                trait_args: method
                    .trait_args
                    .iter()
                    .map(|arg| method_normalization.normalize(*arg))
                    .collect(),
                trait_const_args: method.trait_const_args.clone(),
                where_predicates: method.where_predicates.clone(),
                is_callable: visible_modules.binary_search(&owner).is_ok()
                    && extension_visibility_allows(method.visibility, owner),
                is_trait_witness: trait_is_visible,
            },
        );
    }
    for value in values {
        let owner = value.def_id.module_id;
        let value_module_is_visible = visible_modules.binary_search(&owner).is_ok();
        if !value_module_is_visible && !extension_visibility_allows(value.visibility, owner) {
            continue;
        }
        let trait_is_visible = value.trait_id.is_some_and(|trait_id| {
            trait_witness_modules.binary_search(&owner).is_ok()
                && trait_declaration_allows(defs, graph, trait_id, module_id)
        });
        if !value_module_is_visible && !trait_is_visible {
            continue;
        }
        if resolver_cache
            .defs(owner)
            .is_none_or(|defs| defs.defs.get(value.def_id.def_id).is_none())
        {
            continue;
        }
        let Some(value_normalization) = resolver_cache.normalization(owner) else {
            continue;
        };
        let target_ty = value_normalization.normalize(value.target_ty);
        visible.insert_associated_value(
            value.impl_id,
            target_ty,
            VisibleExtensionAssociatedValue {
                name: value.name,
                def_id: value.def_id,
                visibility: value.visibility,
                is_accessible: owner == module_id
                    || extension_visibility_allows(value.visibility, owner),
            },
        );
        if trait_is_visible {
            visible.insert_trait_witness_impl(owner, value.impl_id);
        }
    }
    VisibleExtensionsForModule {
        methods: Arc::new(visible),
    }
}

/// Whether a trait's own declared visibility reaches `module_id`.
///
/// Implementations are global facts, but a `pub(root)` or private trait's
/// implementations are facts only where the trait itself is visible. This
/// reads the declaration, never the requester's imports. Builtin traits are
/// public.
fn trait_declaration_allows(
    defs: &dyn ProgramDefsResolver,
    graph: &dyn nia_imports::ModuleGraphLookup,
    trait_id: nia_ids::TraitId,
    module_id: nia_ids::ModuleId,
) -> bool {
    let nia_ids::TraitId::Source(trait_id) = trait_id else {
        return true;
    };
    defs.defs(trait_id.module_id).is_some_and(|defs| {
        defs.defs.get(trait_id.def_id).is_some_and(|def| {
            nia_imports::visibility_allows(def.visibility, graph, trait_id.module_id, module_id)
        })
    })
}

/// Indexes the trait implementations in `witness_modules` whose trait is
/// visible from `module_id`.
pub fn visible_trait_impls_for_module(
    module_id: nia_ids::ModuleId,
    type_store: &TypeStore,
    graph: &dyn nia_imports::ModuleGraphLookup,
    defs: &dyn ProgramDefsResolver,
    witness_modules: &[nia_ids::ModuleId],
    trait_impls: &[ProgramTraitImplSignature],
) -> VisibleTraitImplsForModule {
    let trait_impls = trait_impls
        .iter()
        .filter(|impl_signature| {
            witness_modules.contains(&impl_signature.module_id)
                && trait_declaration_allows(defs, graph, impl_signature.trait_id, module_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    let trait_impl_index =
        nia_item_signatures::ProgramTraitImplIndex::new_with_type_store(&trait_impls, type_store);
    VisibleTraitImplsForModule {
        trait_impls,
        trait_impl_index,
    }
}

/// Inputs for computing source-visible extension modules.
pub struct VisibleExtensionProviderModulesInput<'a> {
    /// Module from which visibility is queried.
    pub module_id: nia_ids::ModuleId,
    /// Module graph used for visibility checks.
    pub graph: &'a dyn nia_imports::ModuleGraphLookup,
    /// Current module's using scope.
    pub using_scope: &'a nia_defs::ModuleUsingScope,
    /// Public surfaces used to follow explicit public module re-exports.
    pub public_surfaces: &'a dyn ProgramPublicSurfaceResolver,
}

/// Returns modules contributing extensions visible in this module's source.
///
/// Visibility is based on this module's own `using` scope and its own selected
/// provider edges. Imported modules' private `using` declarations and provider
/// demands are implementation details of those modules; they do not become
/// imports in the caller. Inherent members declared in a nominal type's owner
/// module are resolved separately through the nominal-owner lookup.
pub fn visible_extension_provider_modules(
    input: VisibleExtensionProviderModulesInput<'_>,
) -> Vec<nia_ids::ModuleId> {
    let context = extension_visibility_context(input);
    extension_visibility_modules(&context)
}

/// Returns the modules whose trait implementations are facts for `module_id`.
///
/// Implementations are global and independent of the caller's imports: the
/// set is the module itself plus the transitive closure of its trait-impl
/// provider edges, which the loader adds by searching the packages that own
/// each demanded type and trait. A `using` never adds or hides an impl.
pub fn visible_trait_impl_modules(
    module_id: nia_ids::ModuleId,
    graph: &dyn nia_imports::ModuleGraphLookup,
) -> Vec<nia_ids::ModuleId> {
    let mut seen = HashSet::from([module_id]);
    let mut queue = VecDeque::from(graph.provider_dependencies(module_id, true));
    while let Some(module) = queue.pop_front() {
        if seen.insert(module) {
            queue.extend(graph.provider_dependencies(module, true));
        }
    }
    let mut modules = seen.into_iter().collect::<Vec<_>>();
    modules.sort();
    modules
}

fn extension_visibility_context<'a>(
    input: VisibleExtensionProviderModulesInput<'a>,
) -> ExtensionVisibilityContext<'a> {
    ExtensionVisibilityContext {
        module_id: input.module_id,
        graph: input.graph,
        using_scope: input.using_scope,
        public_surfaces: input.public_surfaces,
    }
}

/// The modules whose inherent extension methods `context.module_id` sees:
/// itself, the modules it imports, the modules they re-export, the providers
/// a directly imported package's root facade publishes, and the providers
/// the loader selected for its method demands. A demand searches only the
/// providers that this module's own imports publish, through the facade a
/// module or an item is imported from, so importing a type through a facade
/// that publishes the type's methods makes them visible, and what a module
/// sees still depends on its own imports alone.
fn extension_visibility_modules(
    context: &ExtensionVisibilityContext<'_>,
) -> Vec<nia_ids::ModuleId> {
    let mut visible = HashSet::from([context.module_id]);
    let direct_imports = context
        .using_scope
        .modules
        .values()
        .copied()
        .collect::<Vec<_>>();
    follow_publications(context, direct_imports.iter().copied(), &mut visible);
    visible.extend(
        context
            .graph
            .provider_dependencies(context.module_id, false),
    );
    // A package root is the namespace facade for every module in that
    // package. A qualified type path such as `std::HashMap` can therefore
    // reach a type re-exported by the root even when the source imports a
    // sibling module (`std::mem` or `std::process`). Activate only the root
    // facades of the caller's direct imports; following every reachable
    // module would make a package's private re-exports leak across another
    // facade. The root contributes provider edges only when it published them
    // explicitly with `pub using`, preserving the same rule as an ordinary
    // facade.
    let package_roots = direct_imports
        .into_iter()
        .filter_map(|module| context.graph.current_package_root_module(module))
        .collect::<HashSet<_>>();
    for package_root in package_roots {
        visible.extend(published_providers(context, package_root));
    }
    visible.remove(&context.module_id);
    let mut modules = visible.into_iter().collect::<Vec<_>>();
    modules.sort();
    modules
}

/// Returns the modules whose extension methods a `using` of `module` makes
/// visible to `input.module_id`, by the rules of
/// [`visible_extension_provider_modules`]: the module, what it re-exports and
/// publishes, and the providers its package root publishes.
///
/// The providers the loader selects for method demands through the import
/// are not included; the loader records them per import.
pub fn imported_module_extension_modules(
    input: VisibleExtensionProviderModulesInput<'_>,
    module: nia_ids::ModuleId,
) -> Vec<nia_ids::ModuleId> {
    let context = extension_visibility_context(input);
    let mut visible = HashSet::new();
    follow_publications(&context, [module], &mut visible);
    if let Some(package_root) = context.graph.current_package_root_module(module) {
        visible.extend(published_providers(&context, package_root));
    }
    visible.remove(&context.module_id);
    let mut modules = visible.into_iter().collect::<Vec<_>>();
    modules.sort();
    modules
}

/// Adds `modules` to `visible`, with the modules each re-exports and the
/// providers each publishes, transitively, as far as the export scopes allow
/// `context.module_id` to see them.
fn follow_publications(
    context: &ExtensionVisibilityContext<'_>,
    modules: impl IntoIterator<Item = nia_ids::ModuleId>,
    visible: &mut HashSet<nia_ids::ModuleId>,
) {
    let mut pending_modules = modules.into_iter().collect::<VecDeque<_>>();
    while let Some(module_id) = pending_modules.pop_front() {
        if !visible.insert(module_id) {
            continue;
        }
        if let Some(surface) = context.public_surfaces.public_surface(module_id) {
            pending_modules.extend(surface.reexported_modules.iter().filter_map(
                |(name, module)| {
                    surface
                        .module_scopes
                        .get(name)
                        .is_none_or(|scope| scope_allows(context, scope))
                        .then_some(*module)
                },
            ));
        }
        pending_modules.extend(published_providers(context, module_id));
    }
}

/// The providers `module` publishes with `pub using` that
/// `context.module_id` may see.
fn published_providers<'a>(
    context: &'a ExtensionVisibilityContext<'_>,
    module: nia_ids::ModuleId,
) -> impl Iterator<Item = nia_ids::ModuleId> + 'a {
    let surface = context.public_surfaces.public_surface(module);
    surface.into_iter().flat_map(|surface| {
        surface
            .reexported_provider_modules
            .iter()
            .filter(|provider| {
                surface
                    .provider_scopes
                    .get(provider)
                    .is_none_or(|scope| scope_allows(context, scope))
            })
            .copied()
            .collect::<Vec<_>>()
    })
}

fn scope_allows(context: &ExtensionVisibilityContext<'_>, scope: &nia_defs::ExportScope) -> bool {
    scope.allows(&|restriction| {
        nia_imports::access_restriction_allows(restriction, context.graph, context.module_id)
    })
}

struct ExtensionVisibilityContext<'a> {
    module_id: nia_ids::ModuleId,
    graph: &'a dyn nia_imports::ModuleGraphLookup,
    using_scope: &'a nia_defs::ModuleUsingScope,
    public_surfaces: &'a dyn ProgramPublicSurfaceResolver,
}
