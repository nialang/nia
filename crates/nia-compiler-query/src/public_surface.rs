// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{UsingGroupItem, UsingSelector, Visibility};
use nia_defs::{
    DefCollection, DefKind, ModulePublicSurface, ModuleUsing, ModuleUsingScope, PublicItem,
    PublicNamespace, PublicSource, PublicSurfaces, UsingEntry,
};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, ModuleId};
use nia_imports::ImportAliasMap;
use nia_span::Span;

/// Compute every module's exported public surface and per-module using scope.
pub(crate) fn compute_public_surfaces(
    defs_by_module: &[DefCollection],
    imports: &ImportAliasMap,
) -> (
    PublicSurfaces,
    HashMap<ModuleId, ModuleUsingScope>,
    Vec<(ModuleId, Diagnostic)>,
) {
    let mut diagnostics: Vec<(ModuleId, Diagnostic)> = Vec::new();
    let defs_by_id = defs_by_module
        .iter()
        .map(|defs| (defs.module_id, defs))
        .collect::<HashMap<_, _>>();

    let mut surfaces = PublicSurfaces::new();
    for defs in defs_by_module {
        let mut surface = ModulePublicSurface::new(defs.module_id);
        for (def_id, def) in defs.defs.iter() {
            if def.parent.is_some() {
                continue;
            }
            if def.visibility != Visibility::Public {
                continue;
            }
            let Some(namespace) = namespace_for(def.kind) else {
                continue;
            };
            let item = PublicItem {
                target_module: defs.module_id,
                target_def_id: def_id,
                namespace,
                name_span: def.span,
                source: PublicSource::Direct,
                parent_enum: None,
            };
            insert_into_surface(&mut surface, &def.name, item);
        }
        surfaces.insert(surface);
    }

    let max_iterations = defs_by_module
        .iter()
        .map(|defs| defs.module_usings.len())
        .sum::<usize>()
        .saturating_mul(2)
        + 4;

    let mut last_unresolved_count = usize::MAX;
    for _ in 0..max_iterations {
        let mut iteration_changed = false;
        let mut iteration_unresolved = 0usize;
        for defs in defs_by_module {
            for using in &defs.module_usings {
                if using.visibility != Visibility::Public {
                    continue;
                }
                match expand_using(&defs_by_id, defs, using, imports, &surfaces) {
                    UsingExpansion::Resolved(entries) => {
                        let surface = surfaces
                            .get(defs.module_id)
                            .cloned()
                            .unwrap_or_else(|| ModulePublicSurface::new(defs.module_id));
                        let mut surface = surface;
                        for entry in entries {
                            match entry.kind {
                                ResolvedEntryKind::Module(target_module) => {
                                    if surface.modules.contains_key(&entry.name) {
                                        continue;
                                    }
                                    iteration_changed = true;
                                    surface.modules.insert(entry.name, target_module);
                                }
                                ResolvedEntryKind::Item(item) => {
                                    if entry_already_present(&surface, &entry.name, item.namespace)
                                    {
                                        continue;
                                    }
                                    iteration_changed = true;
                                    insert_into_surface(&mut surface, &entry.name, item);
                                }
                            }
                        }
                        surfaces.insert(surface);
                    }
                    UsingExpansion::Unresolved => {
                        iteration_unresolved += 1;
                    }
                    UsingExpansion::HardError(diag) => {
                        diagnostics.push((defs.module_id, diag));
                    }
                }
            }
        }
        if !iteration_changed {
            if iteration_unresolved > 0 && iteration_unresolved == last_unresolved_count {
                break;
            }
            if iteration_unresolved == 0 {
                break;
            }
        }
        last_unresolved_count = iteration_unresolved;
    }

    // Final pass: any still-unresolved pub using is a missing item or cycle.
    for defs in defs_by_module {
        for using in &defs.module_usings {
            if using.visibility != Visibility::Public {
                continue;
            }
            match expand_using(&defs_by_id, defs, using, imports, &surfaces) {
                UsingExpansion::Resolved(_) | UsingExpansion::HardError(_) => {}
                UsingExpansion::Unresolved => {
                    diagnostics.push((
                        defs.module_id,
                        Diagnostic::error(
                            using.span,
                            format!(
                                "`pub using {}::...` could not be resolved; possible re-export cycle or unknown name",
                                using.host.first().map(|seg| seg.name.as_str()).unwrap_or("")
                            ),
                        ),
                    ));
                }
            }
        }
    }

    // Now compute per-module using scopes (both pub and non-pub directives).
    let mut using_scopes: HashMap<ModuleId, ModuleUsingScope> = HashMap::new();
    for defs in defs_by_module {
        let mut scope = ModuleUsingScope::default();
        for using in &defs.module_usings {
            let entries = match expand_using(&defs_by_id, defs, using, imports, &surfaces) {
                UsingExpansion::Resolved(entries) => entries,
                UsingExpansion::Unresolved => {
                    if using.visibility != Visibility::Public {
                        diagnostics.push((
                            defs.module_id,
                            Diagnostic::error(
                                using.span,
                                format!(
                                    "`using {}::...` could not be resolved",
                                    using
                                        .host
                                        .first()
                                        .map(|seg| seg.name.as_str())
                                        .unwrap_or("")
                                ),
                            ),
                        ));
                    }
                    continue;
                }
                UsingExpansion::HardError(diag) => {
                    if using.visibility != Visibility::Public {
                        diagnostics.push((defs.module_id, diag));
                    }
                    continue;
                }
            };
            for entry in entries {
                match entry.kind {
                    ResolvedEntryKind::Module(target_module) => {
                        if let Some(previous) =
                            scope.modules.insert(entry.name.clone(), target_module)
                        {
                            diagnostics.push((
                                defs.module_id,
                                Diagnostic::error(
                                    entry.name_span,
                                    format!(
                                        "duplicate using module `{}` in this module",
                                        entry.name
                                    ),
                                ),
                            ));
                            let _ = previous;
                        }
                    }
                    ResolvedEntryKind::Item(item) => {
                        let using_entry = UsingEntry {
                            target_module: item.target_module,
                            target_def_id: item.target_def_id,
                            namespace: item.namespace,
                            directive_span: using.span,
                            name_span: entry.name_span,
                            parent_enum: item.parent_enum,
                        };
                        let table = match item.namespace {
                            PublicNamespace::Value => &mut scope.values,
                            PublicNamespace::Type => &mut scope.types,
                        };
                        if let Some(previous) = table.insert(entry.name.clone(), using_entry) {
                            diagnostics.push((
                                defs.module_id,
                                Diagnostic::error(
                                    entry.name_span,
                                    format!("duplicate using name `{}` in this module", entry.name),
                                ),
                            ));
                            let _ = previous;
                        }
                    }
                }
            }
        }
        using_scopes.insert(defs.module_id, scope);
    }

    (surfaces, using_scopes, diagnostics)
}

fn namespace_for(kind: DefKind) -> Option<PublicNamespace> {
    match kind {
        DefKind::Function | DefKind::Global | DefKind::Comptime => Some(PublicNamespace::Value),
        DefKind::Struct | DefKind::Union | DefKind::Trait | DefKind::Enum | DefKind::TypeAlias => {
            Some(PublicNamespace::Type)
        }
        DefKind::Import
        | DefKind::Method
        | DefKind::TraitMethod
        | DefKind::TraitAssociatedType
        | DefKind::StructField
        | DefKind::UnionField
        | DefKind::EnumVariant => None,
    }
}

fn insert_into_surface(surface: &mut ModulePublicSurface, name: &str, item: PublicItem) {
    let table = match item.namespace {
        PublicNamespace::Value => &mut surface.values,
        PublicNamespace::Type => &mut surface.types,
    };
    table.entry(name.to_string()).or_insert(item);
}

fn entry_already_present(
    surface: &ModulePublicSurface,
    name: &str,
    namespace: PublicNamespace,
) -> bool {
    let table = match namespace {
        PublicNamespace::Value => &surface.values,
        PublicNamespace::Type => &surface.types,
    };
    table.contains_key(name)
}

#[derive(Clone)]
enum ResolvedEntryKind {
    Module(ModuleId),
    Item(PublicItem),
}

#[derive(Clone)]
struct ResolvedEntry {
    name: String,
    name_span: Span,
    kind: ResolvedEntryKind,
}

enum UsingExpansion {
    Resolved(Vec<ResolvedEntry>),
    Unresolved,
    HardError(Diagnostic),
}

#[derive(Clone)]
enum ResolvedNamespace {
    Module(ModuleId),
    Enum(GlobalDefId),
}

fn resolve_namespace_path(
    defs_by_module: &HashMap<ModuleId, &DefCollection>,
    current: &DefCollection,
    imports: &ImportAliasMap,
    surfaces: &PublicSurfaces,
    path: &[nia_ast::UsingHostSegment],
) -> Result<ResolvedNamespace, Diagnostic> {
    let Some(first) = path.first() else {
        return Err(Diagnostic::error(
            Span::default(),
            "`using` requires a namespace path",
        ));
    };
    let mut namespace = if let Some(import) = imports.get(current.module_id, &first.name) {
        ResolvedNamespace::Module(import.target)
    } else if let Some(def_id) = current.module_scope.types.get(&first.name)
        && let Some(def) = current.defs.get(def_id)
        && def.kind == DefKind::Enum
    {
        ResolvedNamespace::Enum(GlobalDefId {
            module_id: current.module_id,
            def_id,
        })
    } else {
        return Err(Diagnostic::error(
            first.span,
            format!(
                "`using {}::...` requires `{0}` to be an imported module alias or a local enum",
                first.name
            ),
        ));
    };

    for segment in &path[1..] {
        namespace = match namespace {
            ResolvedNamespace::Module(module_id) => {
                let Some(surface) = surfaces.get(module_id) else {
                    return Err(Diagnostic::error(
                        segment.span,
                        "module namespace refers to an unresolved public surface",
                    ));
                };
                if let Some(target_module) = surface.lookup_module(&segment.name) {
                    ResolvedNamespace::Module(target_module)
                } else if let Some(item) = surface.lookup_type(&segment.name) {
                    let enum_id = GlobalDefId {
                        module_id: item.target_module,
                        def_id: item.target_def_id,
                    };
                    let Some(target_defs) = defs_by_module.get(&enum_id.module_id).copied() else {
                        return Err(Diagnostic::error(
                            segment.span,
                            "type namespace refers to an unloaded module",
                        ));
                    };
                    let Some(def) = target_defs.defs.get(enum_id.def_id) else {
                        return Err(Diagnostic::error(segment.span, "type definition not found"));
                    };
                    if def.kind != DefKind::Enum {
                        return Err(Diagnostic::error(
                            segment.span,
                            format!("`{}` is not an enum namespace", segment.name),
                        ));
                    }
                    ResolvedNamespace::Enum(enum_id)
                } else {
                    return Err(Diagnostic::error(
                        segment.span,
                        format!("unknown namespace `{}`", segment.name),
                    ));
                }
            }
            ResolvedNamespace::Enum(_) => {
                return Err(Diagnostic::error(
                    segment.span,
                    "enum namespaces do not contain nested namespaces",
                ));
            }
        };
    }
    Ok(namespace)
}

fn expand_namespace(
    namespace: ResolvedNamespace,
    selector: &UsingSelector,
    defs_by_module: &HashMap<ModuleId, &DefCollection>,
    surfaces: &PublicSurfaces,
    source: PublicSource,
) -> UsingExpansion {
    match namespace {
        ResolvedNamespace::Module(target_module) => expand_module_host(
            defs_by_module,
            target_module,
            selector,
            surfaces,
            source.clone(),
        ),
        ResolvedNamespace::Enum(enum_id) => {
            expand_enum_host(enum_id, defs_by_module, selector, false, source.clone())
        }
    }
}

fn expand_using(
    defs_by_module: &HashMap<ModuleId, &DefCollection>,
    current: &DefCollection,
    using: &ModuleUsing,
    imports: &ImportAliasMap,
    surfaces: &PublicSurfaces,
) -> UsingExpansion {
    let source = PublicSource::PubUsing {
        directive_span: using.span,
    };
    if using.host.is_empty() {
        let UsingSelector::Group(items) = &using.selector else {
            return UsingExpansion::Unresolved;
        };
        return expand_root_group(
            defs_by_module,
            current,
            imports,
            surfaces,
            items,
            source.clone(),
        );
    }
    let namespace =
        match resolve_namespace_path(defs_by_module, current, imports, surfaces, &using.host) {
            Ok(namespace) => namespace,
            Err(diag) => return UsingExpansion::HardError(diag),
        };
    if matches!(using.selector, UsingSelector::SelfName) {
        let Some(name) = using.host.last() else {
            return UsingExpansion::Unresolved;
        };
        return expand_self_namespace(name.name.clone(), name.span, namespace, source.clone());
    }
    expand_namespace(
        namespace,
        &using.selector,
        defs_by_module,
        surfaces,
        source.clone(),
    )
}

fn expand_root_group(
    defs_by_module: &HashMap<ModuleId, &DefCollection>,
    current: &DefCollection,
    imports: &ImportAliasMap,
    surfaces: &PublicSurfaces,
    items: &[UsingGroupItem],
    source: PublicSource,
) -> UsingExpansion {
    let mut entries = Vec::new();
    let mut any_unresolved = false;
    for item in items {
        match expand_root_group_item(
            defs_by_module,
            current,
            imports,
            surfaces,
            item,
            source.clone(),
        ) {
            UsingExpansion::Resolved(sub) => entries.extend(sub),
            UsingExpansion::Unresolved => any_unresolved = true,
            UsingExpansion::HardError(diag) => return UsingExpansion::HardError(diag),
        }
    }
    if any_unresolved {
        UsingExpansion::Unresolved
    } else {
        UsingExpansion::Resolved(entries)
    }
}

fn expand_root_group_item(
    defs_by_module: &HashMap<ModuleId, &DefCollection>,
    current: &DefCollection,
    imports: &ImportAliasMap,
    surfaces: &PublicSurfaces,
    item: &UsingGroupItem,
    source: PublicSource,
) -> UsingExpansion {
    match item {
        UsingGroupItem::Name(name) => {
            if let Some(import) = imports.get(current.module_id, &name.name) {
                return UsingExpansion::Resolved(vec![ResolvedEntry {
                    name: name.alias.clone().unwrap_or_else(|| name.name.clone()),
                    name_span: name.alias_span.unwrap_or(name.name_span),
                    kind: ResolvedEntryKind::Module(import.target),
                }]);
            }
            resolve_current_single(current, name, source.clone())
        }
        UsingGroupItem::Nested { host, selector } => {
            let namespace =
                match resolve_namespace_path(defs_by_module, current, imports, surfaces, host) {
                    Ok(namespace) => namespace,
                    Err(diag) => return UsingExpansion::HardError(diag),
                };
            if matches!(selector.as_ref(), UsingSelector::SelfName) {
                let Some(name) = host.last() else {
                    return UsingExpansion::Unresolved;
                };
                return expand_self_namespace(
                    name.name.clone(),
                    name.span,
                    namespace,
                    source.clone(),
                );
            }
            expand_namespace(
                namespace,
                selector,
                defs_by_module,
                surfaces,
                source.clone(),
            )
        }
    }
}

fn expand_module_host(
    defs_by_module: &HashMap<ModuleId, &DefCollection>,
    target_module: ModuleId,
    selector: &UsingSelector,
    surfaces: &PublicSurfaces,
    source: PublicSource,
) -> UsingExpansion {
    let Some(target_surface) = surfaces.get(target_module) else {
        return UsingExpansion::Unresolved;
    };
    match selector {
        UsingSelector::SelfName => UsingExpansion::Resolved(vec![ResolvedEntry {
            name: "module".to_string(),
            name_span: Span::default(),
            kind: ResolvedEntryKind::Module(target_module),
        }]),
        UsingSelector::Wildcard { .. } => {
            let mut entries = Vec::new();
            for (name, module_id) in &target_surface.modules {
                entries.push(ResolvedEntry {
                    name: name.clone(),
                    name_span: Span::default(),
                    kind: ResolvedEntryKind::Module(*module_id),
                });
            }
            for (name, public_item) in target_surface
                .values
                .iter()
                .chain(target_surface.types.iter())
            {
                entries.push(ResolvedEntry {
                    name: name.clone(),
                    name_span: public_item_name_span(public_item),
                    kind: ResolvedEntryKind::Item(PublicItem {
                        target_module: public_item.target_module,
                        target_def_id: public_item.target_def_id,
                        namespace: public_item.namespace,
                        name_span: public_item.name_span,
                        source: source.clone(),
                        parent_enum: public_item.parent_enum,
                    }),
                });
            }
            UsingExpansion::Resolved(entries)
        }
        UsingSelector::Single(name) => resolve_module_single(target_surface, name, source.clone()),
        UsingSelector::Group(items) => {
            let mut entries = Vec::new();
            let mut any_unresolved = false;
            for item in items {
                match expand_group_item(
                    defs_by_module,
                    target_module,
                    item,
                    surfaces,
                    source.clone(),
                ) {
                    UsingExpansion::Resolved(sub) => entries.extend(sub),
                    UsingExpansion::Unresolved => {
                        any_unresolved = true;
                    }
                    UsingExpansion::HardError(diag) => return UsingExpansion::HardError(diag),
                }
            }
            if any_unresolved {
                UsingExpansion::Unresolved
            } else {
                UsingExpansion::Resolved(entries)
            }
        }
    }
}

fn expand_group_item(
    defs_by_module: &HashMap<ModuleId, &DefCollection>,
    current_module: ModuleId,
    item: &UsingGroupItem,
    surfaces: &PublicSurfaces,
    source: PublicSource,
) -> UsingExpansion {
    match item {
        UsingGroupItem::Name(name) => {
            let Some(surface) = surfaces.get(current_module) else {
                return UsingExpansion::Unresolved;
            };
            resolve_module_single(surface, name, source.clone())
        }
        UsingGroupItem::Nested { host, selector } => {
            let namespace =
                match resolve_public_namespace_path(defs_by_module, current_module, surfaces, host)
                {
                    Ok(namespace) => namespace,
                    Err(diag) => return UsingExpansion::HardError(diag),
                };
            if matches!(selector.as_ref(), UsingSelector::SelfName) {
                let Some(name) = host.last() else {
                    return UsingExpansion::Unresolved;
                };
                return expand_self_namespace(
                    name.name.clone(),
                    name.span,
                    namespace,
                    source.clone(),
                );
            }
            expand_namespace(
                namespace,
                selector,
                defs_by_module,
                surfaces,
                source.clone(),
            )
        }
    }
}

fn expand_self_namespace(
    name: String,
    name_span: Span,
    namespace: ResolvedNamespace,
    source: PublicSource,
) -> UsingExpansion {
    match namespace {
        ResolvedNamespace::Module(module_id) => UsingExpansion::Resolved(vec![ResolvedEntry {
            name,
            name_span,
            kind: ResolvedEntryKind::Module(module_id),
        }]),
        ResolvedNamespace::Enum(enum_id) => UsingExpansion::Resolved(vec![ResolvedEntry {
            name,
            name_span,
            kind: ResolvedEntryKind::Item(PublicItem {
                target_module: enum_id.module_id,
                target_def_id: enum_id.def_id,
                namespace: PublicNamespace::Type,
                name_span,
                source: source.clone(),
                parent_enum: None,
            }),
        }]),
    }
}

fn resolve_public_namespace_path(
    defs_by_module: &HashMap<ModuleId, &DefCollection>,
    start_module: ModuleId,
    surfaces: &PublicSurfaces,
    host: &[nia_ast::UsingHostSegment],
) -> Result<ResolvedNamespace, Diagnostic> {
    let Some(first) = host.first() else {
        return Err(Diagnostic::error(
            Span::default(),
            "nested `using` group host must name a namespace",
        ));
    };
    let mut namespace =
        resolve_public_namespace_segment(defs_by_module, start_module, surfaces, first)?;
    for segment in &host[1..] {
        namespace = match namespace {
            ResolvedNamespace::Module(module_id) => {
                resolve_public_namespace_segment(defs_by_module, module_id, surfaces, segment)?
            }
            ResolvedNamespace::Enum(_) => {
                return Err(Diagnostic::error(
                    segment.span,
                    "enum namespaces do not contain nested namespaces",
                ));
            }
        };
    }
    Ok(namespace)
}

fn resolve_public_namespace_segment(
    defs_by_module: &HashMap<ModuleId, &DefCollection>,
    module_id: ModuleId,
    surfaces: &PublicSurfaces,
    segment: &nia_ast::UsingHostSegment,
) -> Result<ResolvedNamespace, Diagnostic> {
    let Some(surface) = surfaces.get(module_id) else {
        return Err(Diagnostic::error(
            segment.span,
            "module namespace refers to an unresolved public surface",
        ));
    };
    if let Some(target_module) = surface.lookup_module(&segment.name) {
        return Ok(ResolvedNamespace::Module(target_module));
    }
    if let Some(item) = surface.lookup_type(&segment.name) {
        let enum_id = GlobalDefId {
            module_id: item.target_module,
            def_id: item.target_def_id,
        };
        let Some(target_defs) = defs_by_module.get(&enum_id.module_id).copied() else {
            return Err(Diagnostic::error(
                segment.span,
                "type namespace refers to an unloaded module",
            ));
        };
        let Some(def) = target_defs.defs.get(enum_id.def_id) else {
            return Err(Diagnostic::error(segment.span, "type definition not found"));
        };
        if def.kind != DefKind::Enum {
            return Err(Diagnostic::error(
                segment.span,
                format!("`{}` is not an enum namespace", segment.name),
            ));
        }
        return Ok(ResolvedNamespace::Enum(enum_id));
    }
    Err(Diagnostic::error(
        segment.span,
        format!("unknown namespace `{}`", segment.name),
    ))
}

fn resolve_module_single(
    target_surface: &ModulePublicSurface,
    name: &nia_ast::UsingName,
    source: PublicSource,
) -> UsingExpansion {
    let local_name = name.alias.clone().unwrap_or_else(|| name.name.clone());
    let local_span = name.alias_span.unwrap_or(name.name_span);
    let mut entries = Vec::new();
    if let Some(module_id) = target_surface.lookup_module(&name.name) {
        entries.push(ResolvedEntry {
            name: local_name.clone(),
            name_span: local_span,
            kind: ResolvedEntryKind::Module(module_id),
        });
    }
    if let Some(item) = target_surface.lookup_value(&name.name) {
        entries.push(ResolvedEntry {
            name: local_name.clone(),
            name_span: local_span,
            kind: ResolvedEntryKind::Item(PublicItem {
                target_module: item.target_module,
                target_def_id: item.target_def_id,
                namespace: PublicNamespace::Value,
                name_span: local_span,
                source: source.clone(),
                parent_enum: item.parent_enum,
            }),
        });
    }
    if let Some(item) = target_surface.lookup_type(&name.name) {
        entries.push(ResolvedEntry {
            name: local_name,
            name_span: local_span,
            kind: ResolvedEntryKind::Item(PublicItem {
                target_module: item.target_module,
                target_def_id: item.target_def_id,
                namespace: PublicNamespace::Type,
                name_span: local_span,
                source: source.clone(),
                parent_enum: item.parent_enum,
            }),
        });
    }
    if entries.is_empty() {
        return UsingExpansion::Unresolved;
    }
    UsingExpansion::Resolved(entries)
}

fn resolve_current_single(
    current: &DefCollection,
    name: &nia_ast::UsingName,
    source: PublicSource,
) -> UsingExpansion {
    let local_name = name.alias.clone().unwrap_or_else(|| name.name.clone());
    let local_span = name.alias_span.unwrap_or(name.name_span);
    let mut entries = Vec::new();
    if let Some(def_id) = current.module_scope.values.get(&name.name)
        && let Some(def) = current.defs.get(def_id)
        && matches!(
            def.kind,
            DefKind::Function | DefKind::Global | DefKind::Comptime
        )
    {
        entries.push(ResolvedEntry {
            name: local_name.clone(),
            name_span: local_span,
            kind: ResolvedEntryKind::Item(PublicItem {
                target_module: current.module_id,
                target_def_id: def_id,
                namespace: PublicNamespace::Value,
                name_span: local_span,
                source: source.clone(),
                parent_enum: None,
            }),
        });
    }
    if let Some(def_id) = current.module_scope.types.get(&name.name)
        && let Some(def) = current.defs.get(def_id)
        && matches!(
            def.kind,
            DefKind::Struct | DefKind::Union | DefKind::Trait | DefKind::Enum | DefKind::TypeAlias
        )
    {
        entries.push(ResolvedEntry {
            name: local_name,
            name_span: local_span,
            kind: ResolvedEntryKind::Item(PublicItem {
                target_module: current.module_id,
                target_def_id: def_id,
                namespace: PublicNamespace::Type,
                name_span: local_span,
                source: source.clone(),
                parent_enum: None,
            }),
        });
    }
    if entries.is_empty() {
        return UsingExpansion::Unresolved;
    }
    UsingExpansion::Resolved(entries)
}

fn expand_enum_host(
    enum_id: GlobalDefId,
    defs_by_module: &HashMap<ModuleId, &DefCollection>,
    selector: &UsingSelector,
    _visible: bool,
    source: PublicSource,
) -> UsingExpansion {
    let Some(target_defs) = defs_by_module.get(&enum_id.module_id).copied() else {
        return UsingExpansion::Unresolved;
    };
    let Some(enum_scope) = target_defs.scopes.enum_members.get(&enum_id.def_id) else {
        return UsingExpansion::Unresolved;
    };
    match selector {
        UsingSelector::SelfName => UsingExpansion::Resolved(vec![ResolvedEntry {
            name: target_defs
                .defs
                .get(enum_id.def_id)
                .map(|def| def.name.clone())
                .unwrap_or_else(|| "Enum".to_string()),
            name_span: target_defs
                .defs
                .get(enum_id.def_id)
                .map(|def| def.span)
                .unwrap_or_default(),
            kind: ResolvedEntryKind::Item(PublicItem {
                target_module: enum_id.module_id,
                target_def_id: enum_id.def_id,
                namespace: PublicNamespace::Type,
                name_span: target_defs
                    .defs
                    .get(enum_id.def_id)
                    .map(|def| def.span)
                    .unwrap_or_default(),
                source: source.clone(),
                parent_enum: None,
            }),
        }]),
        UsingSelector::Wildcard { .. } => {
            let mut entries = Vec::new();
            for (name, def_id) in enum_scope.variants.entries() {
                entries.push(ResolvedEntry {
                    name: name.to_string(),
                    name_span: target_defs
                        .defs
                        .get(def_id)
                        .map(|def| def.span)
                        .unwrap_or_default(),
                    kind: ResolvedEntryKind::Item(PublicItem {
                        target_module: enum_id.module_id,
                        target_def_id: def_id,
                        namespace: PublicNamespace::Value,
                        name_span: target_defs
                            .defs
                            .get(def_id)
                            .map(|def| def.span)
                            .unwrap_or_default(),
                        source: source.clone(),
                        parent_enum: Some(enum_id),
                    }),
                });
            }
            UsingExpansion::Resolved(entries)
        }
        UsingSelector::Single(name) => {
            resolve_enum_single(enum_id, target_defs, enum_scope, name, source.clone())
        }
        UsingSelector::Group(items) => {
            let mut entries = Vec::new();
            for item in items {
                match expand_enum_group_item(enum_id, target_defs, enum_scope, item, source.clone())
                {
                    UsingExpansion::Resolved(sub) => entries.extend(sub),
                    UsingExpansion::Unresolved => return UsingExpansion::Unresolved,
                    UsingExpansion::HardError(diag) => return UsingExpansion::HardError(diag),
                }
            }
            UsingExpansion::Resolved(entries)
        }
    }
}

fn expand_enum_group_item(
    enum_id: GlobalDefId,
    target_defs: &DefCollection,
    enum_scope: &nia_defs::EnumScope,
    item: &UsingGroupItem,
    source: PublicSource,
) -> UsingExpansion {
    match item {
        UsingGroupItem::Name(name) => {
            resolve_enum_single(enum_id, target_defs, enum_scope, name, source.clone())
        }
        UsingGroupItem::Nested { host, .. } => {
            let span = host.first().map(|segment| segment.span).unwrap_or_default();
            UsingExpansion::HardError(Diagnostic::error(
                span,
                "nested `using` group hosts are only valid under a module host",
            ))
        }
    }
}

fn resolve_enum_single(
    enum_id: GlobalDefId,
    target_defs: &DefCollection,
    enum_scope: &nia_defs::EnumScope,
    name: &nia_ast::UsingName,
    source: PublicSource,
) -> UsingExpansion {
    let local_name = name.alias.clone().unwrap_or_else(|| name.name.clone());
    let local_span = name.alias_span.unwrap_or(name.name_span);
    let Some(variant_def_id) = enum_scope.variants.get(&name.name) else {
        return UsingExpansion::HardError(Diagnostic::error(
            name.name_span,
            format!("unknown enum variant `{}`", name.name),
        ));
    };
    let _ = target_defs;
    UsingExpansion::Resolved(vec![ResolvedEntry {
        name: local_name,
        name_span: local_span,
        kind: ResolvedEntryKind::Item(PublicItem {
            target_module: enum_id.module_id,
            target_def_id: variant_def_id,
            namespace: PublicNamespace::Value,
            name_span: local_span,
            source: source.clone(),
            parent_enum: Some(enum_id),
        }),
    }])
}

fn public_item_name_span(item: &PublicItem) -> Span {
    item.name_span
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_imports::{ModuleGraph, SourcePath, collect_import_aliases};

    fn defs(module_id: ModuleId, source: &str) -> DefCollection {
        let (module, errors) = nia_parser::parse_module(source);
        assert!(errors.is_empty(), "{errors:?}");
        nia_defs::collect_module_defs(module_id, &module)
    }

    fn graph_with_imports(imports: &[(&str, &str)]) -> ModuleGraph {
        let source = imports
            .iter()
            .map(|(alias, path)| format!("import .{path} as {alias};"))
            .collect::<Vec<_>>()
            .join("\n");
        let (module, errors) = nia_parser::parse_module(&source);
        assert!(errors.is_empty(), "{errors:?}");

        let mut graph = ModuleGraph::new(SourcePath::new("main.nia"));
        let root = graph.root();
        let mut diagnostics = Vec::new();
        nia_imports::collect_module_imports(
            &mut graph,
            &mut diagnostics,
            root,
            &SourcePath::new("main.nia"),
            &module,
            &nia_imports::ModuleMap::default(),
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        graph
    }

    #[test]
    fn wildcard_reexports_preserve_item_name_spans_for_duplicate_diagnostics() {
        let main = defs(
            ModuleId(0),
            r#"
import .left as left;
import .right as right;
using { left::*, right::* };
"#,
        );
        let left = defs(ModuleId(1), "pub fn value() i32 { 1 }");
        let right = defs(ModuleId(2), "pub fn value() i32 { 2 }");
        let imports =
            collect_import_aliases(&graph_with_imports(&[("left", "left"), ("right", "right")]));

        let (_, _, diagnostics) = compute_public_surfaces(&[main, left, right], &imports);

        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0]
                .1
                .message
                .contains("duplicate using name `value`")
        );
        assert_ne!(diagnostics[0].1.span, Span::default());
    }
}
