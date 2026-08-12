//! Module-alias environment construction before full selector expansion.

use super::*;

pub(crate) fn collect_module_aliases(
    defs_by_module: &HashMap<ModuleId, &DefCollection>,
    current: &DefCollection,
    graph: &ModuleGraph,
    inherited: &SymbolMap<ModuleId>,
    surfaces: &PublicSurfaces,
    symbols: &dyn SymbolText,
) -> SymbolMap<ModuleId> {
    let mut modules = inherited.clone();
    let context = UsingExpansionContext {
        defs_by_module,
        graph,
        accessing_module: current.module_id,
        surfaces,
        symbols,
        mode: UsingLookupMode::Visible,
    };
    for using in &current.module_usings {
        let visible_modules = modules.clone();
        collect_module_aliases_from_using(&context, current, &visible_modules, using, &mut modules);
    }
    modules
}

fn collect_module_aliases_from_using(
    context: &UsingExpansionContext<'_>,
    current: &DefCollection,
    local_modules: &SymbolMap<ModuleId>,
    using: &ModuleUsing,
    modules: &mut SymbolMap<ModuleId>,
) {
    if using.host.is_empty() {
        let UsingSelector::Group(items) = &using.selector else {
            return;
        };
        for item in items {
            collect_root_group_module_aliases(context, current, local_modules, item, modules);
        }
        return;
    }
    let Some(namespace) =
        resolve_module_alias_namespace(context, current, local_modules, &using.host)
    else {
        return;
    };
    collect_module_aliases_from_selector(
        context,
        namespace,
        using.host.last(),
        &using.selector,
        modules,
    );
}

fn resolve_module_alias_namespace(
    context: &UsingExpansionContext<'_>,
    current: &DefCollection,
    local_modules: &SymbolMap<ModuleId>,
    host: &[UsingPathSegment],
) -> Option<ModuleId> {
    let namespace = resolve_namespace_path(context, current, local_modules, host).ok()?;
    match namespace {
        ResolvedNamespace::Module(module_id) => Some(module_id),
        ResolvedNamespace::Enum(_) => None,
    }
}

fn collect_module_aliases_from_selector(
    context: &UsingExpansionContext<'_>,
    namespace: ModuleId,
    self_name_segment: Option<&UsingPathSegment>,
    selector: &UsingSelector,
    modules: &mut SymbolMap<ModuleId>,
) {
    match selector {
        UsingSelector::SelfName => {
            if let Some(segment) = self_name_segment
                && let Some(name) = path_segment_self_name(segment)
            {
                modules.entry(name).or_insert(namespace);
            }
        }
        UsingSelector::Single(name) => {
            if let Some(module_id) = module_alias_target_for_name(context, namespace, &name.name) {
                modules
                    .entry(name.alias.unwrap_or(name.name))
                    .or_insert(module_id);
            }
        }
        UsingSelector::Group(items) => {
            for item in items {
                collect_module_aliases_from_group_item(context, namespace, item, modules);
            }
        }
        UsingSelector::Wildcard { .. } => {}
    }
}

fn collect_module_aliases_from_group_item(
    context: &UsingExpansionContext<'_>,
    namespace: ModuleId,
    item: &UsingGroupItem,
    modules: &mut SymbolMap<ModuleId>,
) {
    match item {
        UsingGroupItem::Name(name) => {
            if let Some(module_id) = module_alias_target_for_name(context, namespace, &name.name) {
                modules
                    .entry(name.alias.unwrap_or(name.name))
                    .or_insert(module_id);
            }
        }
        UsingGroupItem::Nested { host, selector } => {
            let Ok(namespace) = resolve_public_namespace_path(context, namespace, host) else {
                return;
            };
            let ResolvedNamespace::Module(module_id) = namespace else {
                return;
            };
            collect_module_aliases_from_selector(
                context,
                module_id,
                host.last(),
                selector,
                modules,
            );
        }
    }
}

fn collect_root_group_module_aliases(
    context: &UsingExpansionContext<'_>,
    current: &DefCollection,
    local_modules: &SymbolMap<ModuleId>,
    item: &UsingGroupItem,
    modules: &mut SymbolMap<ModuleId>,
) {
    match item {
        UsingGroupItem::Name(name) => {
            let target = root_module_for_segment(
                context.graph,
                current.module_id,
                &UsingPathSegment {
                    kind: PathSegmentKind::Name(name.name),
                    span: name.name_span,
                },
            )
            .or_else(|| visible_child_module_for_mode(context, current.module_id, &name.name))
            .or_else(|| local_modules.get(&name.name).copied());
            if let Some(module_id) = target {
                modules
                    .entry(name.alias.unwrap_or(name.name))
                    .or_insert(module_id);
            }
        }
        UsingGroupItem::Nested { host, selector } => {
            let Some(namespace) =
                resolve_module_alias_namespace(context, current, local_modules, host)
            else {
                return;
            };
            collect_module_aliases_from_selector(
                context,
                namespace,
                host.last(),
                selector,
                modules,
            );
        }
    }
}

fn module_alias_target_for_name(
    context: &UsingExpansionContext<'_>,
    namespace: ModuleId,
    name: &SymbolId,
) -> Option<ModuleId> {
    visible_child_module(context.graph, context.accessing_module, namespace, name).or_else(|| {
        context
            .surfaces
            .get(namespace)
            .and_then(|surface| surface.lookup_module(name))
    })
}
