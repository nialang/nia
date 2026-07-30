// SPDX-License-Identifier: GPL-3.0-or-later
use std::{borrow::Borrow, collections::HashMap};

use nia_defs::{
    DefCollection, DefKind, ModulePublicSurface, ModuleUsing, ModuleUsingScope, PathSegmentKind,
    PublicItem, PublicNamespace, PublicSource, PublicSurfaces, UsingEntry, UsingGroupItem,
    UsingName, UsingPathSegment, UsingSelector, Visibility,
};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalDefId, ModuleId};
use nia_imports::{
    ModuleGraph, ModuleRootSegment, module_declaration_visibility_allows, visibility_allows,
};
use nia_span::Span;
use nia_symbol::{KnownSymbolText, SymbolId, SymbolMap, SymbolText, symbol_text_or_unresolved};

#[derive(Debug, Clone, PartialEq)]
pub struct PublicSurfaceComputation {
    pub surfaces: PublicSurfaces,
    pub using_scopes: HashMap<ModuleId, ModuleUsingScope>,
    pub diagnostics: Vec<(ModuleId, Diagnostic)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PublicSurfaceExports {
    pub surfaces: PublicSurfaces,
    pub diagnostics: Vec<(ModuleId, Diagnostic)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PublicUsingScopes {
    pub using_scopes: HashMap<ModuleId, ModuleUsingScope>,
    pub diagnostics: Vec<(ModuleId, Diagnostic)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TypeExposureIndex {
    names_by_target: HashMap<GlobalDefId, Vec<SymbolId>>,
}

fn symbol_text(symbols: &dyn SymbolText, symbol: SymbolId) -> String {
    symbol_text_or_unresolved(symbols, symbol)
}

fn path_segment_text(symbols: &dyn SymbolText, segment: &UsingPathSegment) -> String {
    match segment.kind {
        PathSegmentKind::Name(name) => symbol_text(symbols, name),
        PathSegmentKind::Package => "pkg".to_string(),
        PathSegmentKind::Super => "super".to_string(),
        PathSegmentKind::SelfValue => "self".to_string(),
    }
}

fn first_path_segment_text(symbols: &dyn SymbolText, path: &[UsingPathSegment]) -> String {
    path.first()
        .map(|segment| path_segment_text(symbols, segment))
        .unwrap_or_default()
}

fn non_initial_special_segment_diagnostic(
    symbols: &dyn SymbolText,
    segment: &UsingPathSegment,
) -> Diagnostic {
    Diagnostic::user_error_at(
        codes::NAME_RESOLUTION,
        segment.span,
        format!(
            "`{}` can only be used as the first path segment",
            path_segment_text(symbols, segment)
        ),
    )
}

fn path_segment_name(segment: &UsingPathSegment) -> Option<SymbolId> {
    match segment.kind {
        PathSegmentKind::Name(name) => Some(name),
        PathSegmentKind::Package | PathSegmentKind::Super | PathSegmentKind::SelfValue => None,
    }
}

fn path_segment_self_name(segment: &UsingPathSegment) -> Option<SymbolId> {
    match segment.kind {
        PathSegmentKind::Name(name) => Some(name),
        PathSegmentKind::Package | PathSegmentKind::Super | PathSegmentKind::SelfValue => None,
    }
}

fn invalid_self_name_segment(segment: &UsingPathSegment) -> Diagnostic {
    Diagnostic::user_error_at(
        codes::NAME_RESOLUTION,
        segment.span,
        "using `self` requires the host path to end in a named module or type",
    )
}

impl TypeExposureIndex {
    pub fn from_defs_surfaces_and_using_scopes<D: Borrow<DefCollection>>(
        defs_by_module: &[D],
        surfaces: &PublicSurfaces,
        using_scopes: &HashMap<ModuleId, ModuleUsingScope>,
    ) -> Self {
        let mut names_by_target: HashMap<GlobalDefId, Vec<SymbolId>> = HashMap::new();
        for defs in defs_by_module {
            let defs = defs.borrow();
            for (def_id, def) in defs.defs.iter() {
                if !matches!(
                    def.kind,
                    DefKind::Struct | DefKind::Union | DefKind::Enum | DefKind::TypeAlias
                ) {
                    continue;
                }
                names_by_target
                    .entry(GlobalDefId {
                        module_id: defs.module_id,
                        def_id,
                    })
                    .or_default()
                    .push(def.name);
            }
        }
        for surface in surfaces.iter().map(|(_, surface)| surface) {
            for (name, item) in &surface.types {
                names_by_target
                    .entry(GlobalDefId {
                        module_id: item.target_module,
                        def_id: item.target_def_id,
                    })
                    .or_default()
                    .push(*name);
            }
        }
        for using_scope in using_scopes.values() {
            for (name, entry) in &using_scope.types {
                names_by_target
                    .entry(GlobalDefId {
                        module_id: entry.target_module,
                        def_id: entry.target_def_id,
                    })
                    .or_default()
                    .push(*name);
            }
        }
        for names in names_by_target.values_mut() {
            names.sort();
            names.dedup();
        }
        Self { names_by_target }
    }

    pub fn names_for(&self, target: GlobalDefId) -> &[SymbolId] {
        self.names_by_target
            .get(&target)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

/// Compute every module's exported public surface and per-module using scope.
pub fn compute_public_surfaces<D: Borrow<DefCollection>>(
    defs_by_module: &[D],
    graph: &ModuleGraph,
) -> (
    PublicSurfaces,
    HashMap<ModuleId, ModuleUsingScope>,
    Vec<(ModuleId, Diagnostic)>,
) {
    let computation =
        compute_public_surface_computation_with_symbols(defs_by_module, graph, &KnownSymbolText);
    (
        computation.surfaces,
        computation.using_scopes,
        computation.diagnostics,
    )
}

pub fn compute_public_surface_computation<D: Borrow<DefCollection>>(
    defs_by_module: &[D],
    graph: &ModuleGraph,
) -> PublicSurfaceComputation {
    compute_public_surface_computation_with_symbols(defs_by_module, graph, &KnownSymbolText)
}

pub fn compute_public_surface_computation_with_symbols<D: Borrow<DefCollection>>(
    defs_by_module: &[D],
    graph: &ModuleGraph,
    symbols: &dyn SymbolText,
) -> PublicSurfaceComputation {
    let exports = compute_exported_public_surfaces_with_symbols(defs_by_module, graph, symbols);
    let scopes = compute_using_scopes_from_surfaces_with_symbols(
        defs_by_module,
        graph,
        &exports.surfaces,
        symbols,
    );
    let mut diagnostics = exports.diagnostics;
    diagnostics.extend(scopes.diagnostics);
    PublicSurfaceComputation {
        surfaces: exports.surfaces,
        using_scopes: scopes.using_scopes,
        diagnostics,
    }
}

pub fn compute_exported_public_surfaces<D: Borrow<DefCollection>>(
    defs_by_module: &[D],
    graph: &ModuleGraph,
) -> PublicSurfaceExports {
    compute_exported_public_surfaces_with_symbols(defs_by_module, graph, &KnownSymbolText)
}

pub fn compute_exported_public_surfaces_with_symbols<D: Borrow<DefCollection>>(
    defs_by_module: &[D],
    graph: &ModuleGraph,
    symbols: &dyn SymbolText,
) -> PublicSurfaceExports {
    let mut diagnostics: Vec<(ModuleId, Diagnostic)> = Vec::new();
    let defs_by_id = defs_by_module
        .iter()
        .map(|defs| {
            let defs = defs.borrow();
            (defs.module_id, defs)
        })
        .collect::<HashMap<_, _>>();
    let mut surfaces = PublicSurfaces::new();
    for defs in defs_by_module {
        let defs = defs.borrow();
        let mut surface = ModulePublicSurface::new(defs.module_id);
        if let Some(node) = graph.get(defs.module_id) {
            for declaration in &node.declarations {
                if declaration.visibility == Visibility::Public {
                    surface.modules.insert(declaration.name, declaration.target);
                }
            }
        }
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
        .map(|defs| defs.borrow().module_usings.len())
        .sum::<usize>()
        .saturating_mul(2)
        + 4;

    let mut last_unresolved_count = usize::MAX;
    for _ in 0..max_iterations {
        let mut iteration_changed = false;
        let mut iteration_unresolved = 0usize;
        for defs in defs_by_module {
            let defs = defs.borrow();
            let local_modules = collect_module_aliases(
                &defs_by_id,
                defs,
                graph,
                &SymbolMap::default(),
                &surfaces,
                symbols,
            );
            for using in &defs.module_usings {
                if using.visibility != Visibility::Public {
                    continue;
                }
                let context = UsingExpansionContext {
                    defs_by_module: &defs_by_id,
                    graph,
                    accessing_module: defs.module_id,
                    surfaces: &surfaces,
                    symbols,
                    mode: UsingLookupMode::PublicOnly,
                };
                match expand_using(&context, defs, using, &local_modules) {
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
                    UsingExpansion::HardError(_) => {}
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
        let defs = defs.borrow();
        let process_used_paths = graph
            .get(defs.module_id)
            .is_some_and(|node| node.process_used_paths);
        let local_modules = collect_module_aliases(
            &defs_by_id,
            defs,
            graph,
            &SymbolMap::default(),
            &surfaces,
            symbols,
        );
        for using in &defs.module_usings {
            if using.visibility != Visibility::Public {
                continue;
            }
            let context = UsingExpansionContext {
                defs_by_module: &defs_by_id,
                graph,
                accessing_module: defs.module_id,
                surfaces: &surfaces,
                symbols,
                mode: UsingLookupMode::PublicOnly,
            };
            match expand_using(&context, defs, using, &local_modules) {
                UsingExpansion::Resolved(_) | UsingExpansion::HardError(_) => {}
                UsingExpansion::Unresolved
                    if process_used_paths
                        && !using_host_waits_on_unprocessed_module(
                            graph,
                            defs.module_id,
                            &using.host,
                        ) =>
                {
                    diagnostics.push((
                        defs.module_id,
                        Diagnostic::user_error_at(codes::NAME_RESOLUTION,
                            using.span,
                                format!(
                                    "`pub using {}::...` could not be resolved; possible re-export cycle or unknown name",
                                    first_path_segment_text(symbols, &using.host)
                                ),
                        ),
                    ));
                }
                UsingExpansion::Unresolved => {}
            }
        }
    }

    PublicSurfaceExports {
        surfaces,
        diagnostics,
    }
}

fn using_host_waits_on_unprocessed_module(
    graph: &ModuleGraph,
    current_module: ModuleId,
    host: &[UsingPathSegment],
) -> bool {
    let Some(first) = host.first() else {
        return false;
    };
    let Some(mut module_id) = root_module_for_segment(graph, current_module, first) else {
        return false;
    };
    for segment in &host[1..] {
        let Some(module) = graph.get(module_id) else {
            return false;
        };
        if !module.process_used_paths {
            return true;
        }
        let Some(name) = path_segment_name(segment) else {
            return false;
        };
        let Some(next) = module.children.get(&name).copied() else {
            return false;
        };
        module_id = next;
    }
    graph
        .get(module_id)
        .is_some_and(|module| !module.process_used_paths)
}

pub fn compute_using_scopes_from_surfaces<D: Borrow<DefCollection>>(
    defs_by_module: &[D],
    graph: &ModuleGraph,
    surfaces: &PublicSurfaces,
) -> PublicUsingScopes {
    compute_using_scopes_from_surfaces_with_symbols(
        defs_by_module,
        graph,
        surfaces,
        &KnownSymbolText,
    )
}

pub fn compute_using_scopes_from_surfaces_with_symbols<D: Borrow<DefCollection>>(
    defs_by_module: &[D],
    graph: &ModuleGraph,
    surfaces: &PublicSurfaces,
    symbols: &dyn SymbolText,
) -> PublicUsingScopes {
    let mut diagnostics: Vec<(ModuleId, Diagnostic)> = Vec::new();
    let defs_by_id = defs_by_module
        .iter()
        .map(|defs| {
            let defs = defs.borrow();
            (defs.module_id, defs)
        })
        .collect::<HashMap<_, _>>();
    let mut using_scopes: HashMap<ModuleId, ModuleUsingScope> = HashMap::new();
    for defs in defs_by_module {
        let defs = defs.borrow();
        let process_used_paths = graph
            .get(defs.module_id)
            .is_some_and(|node| node.process_used_paths);
        let mut scope = ModuleUsingScope::default();
        for using in &defs.module_usings {
            let mode = if using.visibility == Visibility::Public {
                UsingLookupMode::PublicOnly
            } else {
                UsingLookupMode::Visible
            };
            let context = UsingExpansionContext {
                defs_by_module: &defs_by_id,
                graph,
                accessing_module: defs.module_id,
                surfaces,
                symbols,
                mode,
            };
            let entries = match expand_using(&context, defs, using, &scope.modules) {
                UsingExpansion::Resolved(entries) => entries,
                UsingExpansion::Unresolved => {
                    record_unresolved_using_names(&mut scope, using);
                    if process_used_paths && using.visibility != Visibility::Public {
                        diagnostics.push((
                            defs.module_id,
                            Diagnostic::user_error_at(
                                codes::NAME_RESOLUTION,
                                using.span,
                                format!(
                                    "`using {}::...` could not be resolved",
                                    first_path_segment_text(symbols, &using.host)
                                ),
                            ),
                        ));
                    }
                    continue;
                }
                UsingExpansion::HardError(diag) => {
                    record_unresolved_using_names(&mut scope, using);
                    if process_used_paths && using.visibility != Visibility::Public {
                        diagnostics.push((defs.module_id, diag));
                    }
                    continue;
                }
            };
            for entry in entries {
                match entry.kind {
                    ResolvedEntryKind::Module(target_module) => {
                        if let Some(previous) = scope.modules.get(&entry.name).copied() {
                            if previous == target_module {
                                continue;
                            }
                            diagnostics.push((
                                defs.module_id,
                                Diagnostic::user_error_at(
                                    codes::NAME_RESOLUTION,
                                    entry.name_span,
                                    format!(
                                        "duplicate using module `{}` in this module",
                                        symbol_text(symbols, entry.name)
                                    ),
                                ),
                            ));
                            continue;
                        }
                        scope.modules.insert(entry.name, target_module);
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
                        if let Some(previous) = table.insert(entry.name, using_entry) {
                            diagnostics.push((
                                defs.module_id,
                                Diagnostic::user_error_at(
                                    codes::NAME_RESOLUTION,
                                    entry.name_span,
                                    format!(
                                        "duplicate using name `{}` in this module",
                                        symbol_text(symbols, entry.name)
                                    ),
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

    PublicUsingScopes {
        using_scopes,
        diagnostics,
    }
}

fn namespace_for(kind: DefKind) -> Option<PublicNamespace> {
    match kind {
        DefKind::Function | DefKind::Global | DefKind::Const => Some(PublicNamespace::Value),
        DefKind::Struct | DefKind::Union | DefKind::Trait | DefKind::Enum | DefKind::TypeAlias => {
            Some(PublicNamespace::Type)
        }
        DefKind::Module
        | DefKind::Method
        | DefKind::TraitMethod
        | DefKind::TraitAssociatedType
        | DefKind::StructField
        | DefKind::UnionField
        | DefKind::EnumVariant
        | DefKind::EnumVariantField => None,
    }
}

fn collect_module_aliases(
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

fn insert_into_surface(surface: &mut ModulePublicSurface, name: &SymbolId, item: PublicItem) {
    let table = match item.namespace {
        PublicNamespace::Value => &mut surface.values,
        PublicNamespace::Type => &mut surface.types,
    };
    table.entry(*name).or_insert(item);
}

fn entry_already_present(
    surface: &ModulePublicSurface,
    name: &SymbolId,
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
    name: SymbolId,
    name_span: Span,
    kind: ResolvedEntryKind,
}

enum UsingExpansion {
    Resolved(Vec<ResolvedEntry>),
    Unresolved,
    HardError(Diagnostic),
}

struct UsingExpansionContext<'a> {
    defs_by_module: &'a HashMap<ModuleId, &'a DefCollection>,
    graph: &'a ModuleGraph,
    accessing_module: ModuleId,
    surfaces: &'a PublicSurfaces,
    symbols: &'a dyn SymbolText,
    mode: UsingLookupMode,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UsingLookupMode {
    PublicOnly,
    Visible,
}

#[derive(Clone)]
enum ResolvedNamespace {
    Module(ModuleId),
    Enum(GlobalDefId),
}

fn root_module_for_segment(
    graph: &ModuleGraph,
    current_module: ModuleId,
    segment: &UsingPathSegment,
) -> Option<ModuleId> {
    graph.root_module_for_segment(
        current_module,
        module_root_segment_from_path_segment(segment.kind),
    )
}

fn module_root_segment_from_path_segment(kind: PathSegmentKind) -> ModuleRootSegment {
    match kind {
        PathSegmentKind::SelfValue => ModuleRootSegment::Current,
        PathSegmentKind::Super => ModuleRootSegment::Parent,
        PathSegmentKind::Package => ModuleRootSegment::PackageRelative,
        PathSegmentKind::Name(name) => ModuleRootSegment::Named(name),
    }
}

fn resolve_namespace_path(
    context: &UsingExpansionContext<'_>,
    current: &DefCollection,
    local_modules: &SymbolMap<ModuleId>,
    path: &[UsingPathSegment],
) -> Result<ResolvedNamespace, Diagnostic> {
    let defs_by_module = context.defs_by_module;
    let graph = context.graph;
    let surfaces = context.surfaces;
    let mode = context.mode;
    let symbols = context.symbols;
    let Some(first) = path.first() else {
        return Err(Diagnostic::user_error_at(
            codes::NAME_RESOLUTION,
            Span::default(),
            "`using` requires a namespace path",
        ));
    };
    let mut namespace =
        if let Some(module_id) = root_module_for_segment(graph, current.module_id, first) {
            ResolvedNamespace::Module(module_id)
        } else if let Some(name) = path_segment_name(first)
            && let Some(module_id) = local_modules.get(&name).copied()
        {
            ResolvedNamespace::Module(module_id)
        } else if let Some(name) = path_segment_name(first)
            && let Some(def_id) = current.module_scope.types.get(&name)
            && let Some(def) = current.defs.get(def_id)
            && def.kind == DefKind::Enum
        {
            ResolvedNamespace::Enum(GlobalDefId {
                module_id: current.module_id,
                def_id,
            })
        } else {
            return Err(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                first.span,
                format!(
                    "`using {}::...` requires `{0}` to be a module namespace or a local enum",
                    path_segment_text(symbols, first)
                ),
            ));
        };

    for segment in &path[1..] {
        let Some(segment_name) = path_segment_name(segment) else {
            return Err(non_initial_special_segment_diagnostic(symbols, segment));
        };
        namespace = match namespace {
            ResolvedNamespace::Module(module_id) => {
                let Some(surface) = surfaces.get(module_id) else {
                    return Err(Diagnostic::user_error_at(
                        codes::NAME_RESOLUTION,
                        segment.span,
                        "module namespace refers to an unresolved public surface",
                    ));
                };
                if let Some(target_module) = surface.lookup_module(&segment_name) {
                    ResolvedNamespace::Module(target_module)
                } else if let Some(target_module) =
                    visible_child_module(graph, current.module_id, module_id, &segment_name)
                {
                    ResolvedNamespace::Module(target_module)
                } else if let Some(item) = surface.lookup_type(&segment_name) {
                    let enum_id = GlobalDefId {
                        module_id: item.target_module,
                        def_id: item.target_def_id,
                    };
                    let Some(target_defs) = defs_by_module.get(&enum_id.module_id).copied() else {
                        return Err(Diagnostic::user_error_at(
                            codes::NAME_RESOLUTION,
                            segment.span,
                            "type namespace refers to an unloaded module",
                        ));
                    };
                    let Some(def) = target_defs.defs.get(enum_id.def_id) else {
                        return Err(Diagnostic::user_error_at(
                            codes::NAME_RESOLUTION,
                            segment.span,
                            "type definition not found",
                        ));
                    };
                    if def.kind != DefKind::Enum {
                        return Err(Diagnostic::user_error_at(
                            codes::NAME_RESOLUTION,
                            segment.span,
                            format!(
                                "`{}` is not an enum namespace",
                                symbol_text(symbols, segment_name)
                            ),
                        ));
                    }
                    ResolvedNamespace::Enum(enum_id)
                } else if let Some(enum_id) = visible_direct_enum_namespace(
                    defs_by_module,
                    graph,
                    current.module_id,
                    module_id,
                    &segment_name,
                    mode,
                ) {
                    ResolvedNamespace::Enum(enum_id)
                } else {
                    return Err(Diagnostic::user_error_at(
                        codes::NAME_RESOLUTION,
                        segment.span,
                        format!("unknown namespace `{}`", symbol_text(symbols, segment_name)),
                    ));
                }
            }
            ResolvedNamespace::Enum(_) => {
                return Err(Diagnostic::user_error_at(
                    codes::NAME_RESOLUTION,
                    segment.span,
                    "enum namespaces do not contain nested namespaces",
                ));
            }
        };
    }
    Ok(namespace)
}

fn visible_child_module(
    graph: &ModuleGraph,
    accessing_module: ModuleId,
    parent_module: ModuleId,
    name: &SymbolId,
) -> Option<ModuleId> {
    let parent = graph.get(parent_module)?;
    let target = parent.children.get(name).copied()?;
    let declaration = parent
        .declarations
        .iter()
        .find(|declaration| &declaration.name == name && declaration.target == target)?;
    module_declaration_visibility_allows(
        declaration.visibility,
        graph,
        parent_module,
        accessing_module,
    )
    .then_some(target)
}

fn module_declaration_visible_for_wildcard(
    mode: UsingLookupMode,
    visibility: Visibility,
    graph: &ModuleGraph,
    declaring_module: ModuleId,
    accessing_module: ModuleId,
) -> bool {
    match mode {
        UsingLookupMode::PublicOnly => visibility == Visibility::Public,
        UsingLookupMode::Visible => module_declaration_visibility_allows(
            visibility,
            graph,
            declaring_module,
            accessing_module,
        ),
    }
}

fn item_visibility_allows(
    mode: UsingLookupMode,
    graph: &ModuleGraph,
    defining_module: ModuleId,
    accessing_module: ModuleId,
    visibility: Visibility,
) -> bool {
    match mode {
        UsingLookupMode::PublicOnly => visibility == Visibility::Public,
        UsingLookupMode::Visible => {
            defining_module == accessing_module
                || visibility_allows(visibility, graph, defining_module, accessing_module)
        }
    }
}

fn direct_item_entry(
    defs: &DefCollection,
    context: &UsingExpansionContext<'_>,
    def_id: nia_defs::DefId,
    local_name: SymbolId,
    local_span: Span,
    source: PublicSource,
) -> Option<ResolvedEntry> {
    let def = defs.defs.get(def_id)?;
    let namespace = namespace_for(def.kind)?;
    if !item_visibility_allows(
        context.mode,
        context.graph,
        defs.module_id,
        context.accessing_module,
        def.visibility,
    ) {
        return None;
    }
    Some(ResolvedEntry {
        name: local_name,
        name_span: local_span,
        kind: ResolvedEntryKind::Item(PublicItem {
            target_module: defs.module_id,
            target_def_id: def_id,
            namespace,
            name_span: local_span,
            source,
            parent_enum: None,
        }),
    })
}

fn visible_direct_enum_namespace(
    defs_by_module: &HashMap<ModuleId, &DefCollection>,
    graph: &ModuleGraph,
    accessing_module: ModuleId,
    module_id: ModuleId,
    name: &SymbolId,
    mode: UsingLookupMode,
) -> Option<GlobalDefId> {
    let target_defs = defs_by_module.get(&module_id).copied()?;
    let def_id = target_defs.module_scope.types.get(name)?;
    let def = target_defs.defs.get(def_id)?;
    if def.kind != DefKind::Enum
        || !item_visibility_allows(mode, graph, module_id, accessing_module, def.visibility)
    {
        return None;
    }
    Some(GlobalDefId { module_id, def_id })
}

fn expand_namespace(
    namespace: ResolvedNamespace,
    selector: &UsingSelector,
    context: &UsingExpansionContext<'_>,
    source: PublicSource,
) -> UsingExpansion {
    match namespace {
        ResolvedNamespace::Module(target_module) => {
            expand_module_host(context, target_module, selector, source.clone())
        }
        ResolvedNamespace::Enum(enum_id) => expand_enum_host(
            enum_id,
            context.defs_by_module,
            selector,
            false,
            context.symbols,
            source.clone(),
        ),
    }
}

fn expand_using(
    context: &UsingExpansionContext<'_>,
    current: &DefCollection,
    using: &ModuleUsing,
    local_modules: &SymbolMap<ModuleId>,
) -> UsingExpansion {
    let source = PublicSource::PubUsing {
        directive_span: using.span,
    };
    if using.host.is_empty() {
        let UsingSelector::Group(items) = &using.selector else {
            return UsingExpansion::Unresolved;
        };
        return expand_root_group(context, current, local_modules, items, source.clone());
    }
    let namespace = match resolve_namespace_path(context, current, local_modules, &using.host) {
        Ok(namespace) => namespace,
        Err(diag) => return UsingExpansion::HardError(diag),
    };
    if matches!(using.selector, UsingSelector::SelfName) {
        let Some(name) = using.host.last() else {
            return UsingExpansion::Unresolved;
        };
        let Some(name_symbol) = path_segment_self_name(name) else {
            return UsingExpansion::HardError(invalid_self_name_segment(name));
        };
        return expand_self_namespace(name_symbol, name.span, namespace, source.clone());
    }
    expand_namespace(namespace, &using.selector, context, source.clone())
}

fn record_unresolved_using_names(scope: &mut ModuleUsingScope, using: &ModuleUsing) {
    let mut names = Vec::new();
    collect_explicit_using_names(&using.host, &using.selector, &mut names);
    scope.unresolved_names.extend(names);
}

fn collect_explicit_using_names(
    host: &[UsingPathSegment],
    selector: &UsingSelector,
    names: &mut Vec<SymbolId>,
) {
    match selector {
        UsingSelector::Single(name) => {
            names.push(name.alias.unwrap_or(name.name));
        }
        UsingSelector::SelfName => {
            if let Some(segment) = host.last()
                && let Some(name) = path_segment_self_name(segment)
            {
                names.push(name);
            }
        }
        UsingSelector::Group(items) => {
            for item in items {
                match item {
                    UsingGroupItem::Name(name) => {
                        names.push(name.alias.unwrap_or(name.name));
                    }
                    UsingGroupItem::Nested { host, selector } => {
                        collect_explicit_using_names(host, selector, names);
                    }
                }
            }
        }
        UsingSelector::Wildcard { .. } => {}
    }
}

fn expand_root_group(
    context: &UsingExpansionContext<'_>,
    current: &DefCollection,
    local_modules: &SymbolMap<ModuleId>,
    items: &[UsingGroupItem],
    source: PublicSource,
) -> UsingExpansion {
    let mut entries = Vec::new();
    let mut any_unresolved = false;
    for item in items {
        match expand_root_group_item(context, current, local_modules, item, source.clone()) {
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
    context: &UsingExpansionContext<'_>,
    current: &DefCollection,
    local_modules: &SymbolMap<ModuleId>,
    item: &UsingGroupItem,
    source: PublicSource,
) -> UsingExpansion {
    match item {
        UsingGroupItem::Name(name) => {
            if let Some(module_id) = root_module_for_segment(
                context.graph,
                current.module_id,
                &UsingPathSegment {
                    kind: PathSegmentKind::Name(name.name),
                    span: name.name_span,
                },
            ) {
                return UsingExpansion::Resolved(vec![ResolvedEntry {
                    name: name.alias.unwrap_or(name.name),
                    name_span: name.alias_span.unwrap_or(name.name_span),
                    kind: ResolvedEntryKind::Module(module_id),
                }]);
            }
            if let Some(module_id) =
                visible_child_module_for_mode(context, current.module_id, &name.name)
            {
                return UsingExpansion::Resolved(vec![ResolvedEntry {
                    name: name.alias.unwrap_or(name.name),
                    name_span: name.alias_span.unwrap_or(name.name_span),
                    kind: ResolvedEntryKind::Module(module_id),
                }]);
            }
            if let Some(module_id) = local_modules.get(&name.name).copied() {
                return UsingExpansion::Resolved(vec![ResolvedEntry {
                    name: name.alias.unwrap_or(name.name),
                    name_span: name.alias_span.unwrap_or(name.name_span),
                    kind: ResolvedEntryKind::Module(module_id),
                }]);
            }
            resolve_current_single(current, name, source.clone())
        }
        UsingGroupItem::Nested { host, selector } => {
            let namespace = match resolve_namespace_path(context, current, local_modules, host) {
                Ok(namespace) => namespace,
                Err(diag) => return UsingExpansion::HardError(diag),
            };
            if matches!(selector.as_ref(), UsingSelector::SelfName) {
                let Some(name) = host.last() else {
                    return UsingExpansion::Unresolved;
                };
                let Some(name_symbol) = path_segment_self_name(name) else {
                    return UsingExpansion::HardError(invalid_self_name_segment(name));
                };
                return expand_self_namespace(name_symbol, name.span, namespace, source.clone());
            }
            expand_namespace(namespace, selector, context, source.clone())
        }
    }
}

fn visible_child_module_for_mode(
    context: &UsingExpansionContext<'_>,
    parent_module: ModuleId,
    name: &SymbolId,
) -> Option<ModuleId> {
    let parent = context.graph.get(parent_module)?;
    let target = parent.children.get(name).copied()?;
    let declaration = parent
        .declarations
        .iter()
        .find(|declaration| &declaration.name == name && declaration.target == target)?;
    module_declaration_visible_for_wildcard(
        context.mode,
        declaration.visibility,
        context.graph,
        parent_module,
        context.accessing_module,
    )
    .then_some(target)
}

fn expand_module_host(
    context: &UsingExpansionContext<'_>,
    target_module: ModuleId,
    selector: &UsingSelector,
    source: PublicSource,
) -> UsingExpansion {
    let Some(target_surface) = context.surfaces.get(target_module) else {
        return UsingExpansion::Unresolved;
    };
    match selector {
        UsingSelector::SelfName => UsingExpansion::Unresolved,
        UsingSelector::Wildcard { .. } => {
            let mut entries = Vec::new();
            if let Some(parent) = context.graph.get(target_module) {
                for declaration in &parent.declarations {
                    if module_declaration_visible_for_wildcard(
                        context.mode,
                        declaration.visibility,
                        context.graph,
                        target_module,
                        context.accessing_module,
                    ) {
                        entries.push(ResolvedEntry {
                            name: declaration.name,
                            name_span: declaration.span,
                            kind: ResolvedEntryKind::Module(declaration.target),
                        });
                    }
                }
            }
            for (name, module_id) in &target_surface.modules {
                if entries
                    .iter()
                    .any(|entry| matches!(entry.kind, ResolvedEntryKind::Module(existing) if existing == *module_id) && entry.name == *name)
                {
                    continue;
                }
                entries.push(ResolvedEntry {
                    name: *name,
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
                    name: *name,
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
            if let Some(target_defs) = context.defs_by_module.get(&target_module).copied() {
                for (name, def_id) in target_defs
                    .module_scope
                    .values
                    .entries()
                    .chain(target_defs.module_scope.types.entries())
                {
                    let Some(def) = target_defs.defs.get(def_id) else {
                        continue;
                    };
                    let Some(namespace) = namespace_for(def.kind) else {
                        continue;
                    };
                    if entries.iter().any(|entry| {
                        &entry.name == name
                            && matches!(
                                &entry.kind,
                                ResolvedEntryKind::Item(item) if item.namespace == namespace
                            )
                    }) {
                        continue;
                    }
                    if let Some(entry) = direct_item_entry(
                        target_defs,
                        context,
                        def_id,
                        *name,
                        def.span,
                        source.clone(),
                    ) {
                        entries.push(entry);
                    }
                }
            }
            UsingExpansion::Resolved(entries)
        }
        UsingSelector::Single(name) => {
            if let Some(module_id) = visible_child_module(
                context.graph,
                context.accessing_module,
                target_module,
                &name.name,
            ) {
                return UsingExpansion::Resolved(vec![ResolvedEntry {
                    name: name.alias.unwrap_or(name.name),
                    name_span: name.alias_span.unwrap_or(name.name_span),
                    kind: ResolvedEntryKind::Module(module_id),
                }]);
            }
            resolve_module_single(context, target_surface, target_module, name, source.clone())
        }
        UsingSelector::Group(items) => {
            let mut entries = Vec::new();
            let mut any_unresolved = false;
            for item in items {
                match expand_group_item(context, target_module, item, source.clone()) {
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
    context: &UsingExpansionContext<'_>,
    current_module: ModuleId,
    item: &UsingGroupItem,
    source: PublicSource,
) -> UsingExpansion {
    match item {
        UsingGroupItem::Name(name) => {
            if let Some(module_id) = visible_child_module(
                context.graph,
                context.accessing_module,
                current_module,
                &name.name,
            ) {
                return UsingExpansion::Resolved(vec![ResolvedEntry {
                    name: name.alias.unwrap_or(name.name),
                    name_span: name.alias_span.unwrap_or(name.name_span),
                    kind: ResolvedEntryKind::Module(module_id),
                }]);
            }
            let Some(surface) = context.surfaces.get(current_module) else {
                return UsingExpansion::Unresolved;
            };
            resolve_module_single(context, surface, current_module, name, source.clone())
        }
        UsingGroupItem::Nested { host, selector } => {
            let namespace = match resolve_public_namespace_path(context, current_module, host) {
                Ok(namespace) => namespace,
                Err(diag) => return UsingExpansion::HardError(diag),
            };
            if matches!(selector.as_ref(), UsingSelector::SelfName) {
                let Some(name) = host.last() else {
                    return UsingExpansion::Unresolved;
                };
                let Some(name_symbol) = path_segment_self_name(name) else {
                    return UsingExpansion::HardError(invalid_self_name_segment(name));
                };
                return expand_self_namespace(name_symbol, name.span, namespace, source.clone());
            }
            expand_namespace(namespace, selector, context, source.clone())
        }
    }
}

fn expand_self_namespace(
    name: SymbolId,
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
    context: &UsingExpansionContext<'_>,
    start_module: ModuleId,
    host: &[UsingPathSegment],
) -> Result<ResolvedNamespace, Diagnostic> {
    let Some(first) = host.first() else {
        return Err(Diagnostic::user_error_at(
            codes::NAME_RESOLUTION,
            Span::default(),
            "nested `using` group host must name a namespace",
        ));
    };
    let mut namespace = resolve_public_namespace_segment(context, start_module, first)?;
    for segment in &host[1..] {
        if !matches!(segment.kind, PathSegmentKind::Name(_)) {
            return Err(non_initial_special_segment_diagnostic(
                context.symbols,
                segment,
            ));
        }
        namespace = match namespace {
            ResolvedNamespace::Module(module_id) => {
                resolve_public_namespace_segment(context, module_id, segment)?
            }
            ResolvedNamespace::Enum(_) => {
                return Err(Diagnostic::user_error_at(
                    codes::NAME_RESOLUTION,
                    segment.span,
                    "enum namespaces do not contain nested namespaces",
                ));
            }
        };
    }
    Ok(namespace)
}

fn resolve_public_namespace_segment(
    context: &UsingExpansionContext<'_>,
    module_id: ModuleId,
    segment: &UsingPathSegment,
) -> Result<ResolvedNamespace, Diagnostic> {
    let defs_by_module = context.defs_by_module;
    let graph = context.graph;
    let accessing_module = context.accessing_module;
    let surfaces = context.surfaces;
    let mode = context.mode;
    let symbols = context.symbols;
    let Some(segment_name) = path_segment_name(segment) else {
        return match segment.kind {
            PathSegmentKind::SelfValue => Ok(ResolvedNamespace::Module(module_id)),
            PathSegmentKind::Super => graph
                .get(module_id)
                .and_then(|node| node.parent)
                .map(ResolvedNamespace::Module)
                .ok_or_else(|| {
                    Diagnostic::user_error_at(
                        codes::NAME_RESOLUTION,
                        segment.span,
                        "`super` has no parent module",
                    )
                }),
            PathSegmentKind::Package => graph
                .current_package_root(module_id)
                .map(ResolvedNamespace::Module)
                .ok_or_else(|| {
                    Diagnostic::user_error_at(
                        codes::NAME_RESOLUTION,
                        segment.span,
                        "`pkg` has no package root",
                    )
                }),
            PathSegmentKind::Name(_) => unreachable!(),
        };
    };
    let Some(surface) = surfaces.get(module_id) else {
        return Err(Diagnostic::user_error_at(
            codes::NAME_RESOLUTION,
            segment.span,
            "module namespace refers to an unresolved public surface",
        ));
    };
    if let Some(target_module) = surface.lookup_module(&segment_name) {
        return Ok(ResolvedNamespace::Module(target_module));
    }
    if let Some(target_module) =
        visible_child_module(graph, accessing_module, module_id, &segment_name)
    {
        return Ok(ResolvedNamespace::Module(target_module));
    }
    if let Some(item) = surface.lookup_type(&segment_name) {
        let enum_id = GlobalDefId {
            module_id: item.target_module,
            def_id: item.target_def_id,
        };
        let Some(target_defs) = defs_by_module.get(&enum_id.module_id).copied() else {
            return Err(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                segment.span,
                "type namespace refers to an unloaded module",
            ));
        };
        let Some(def) = target_defs.defs.get(enum_id.def_id) else {
            return Err(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                segment.span,
                "type definition not found",
            ));
        };
        if def.kind != DefKind::Enum {
            return Err(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                segment.span,
                format!(
                    "`{}` is not an enum namespace",
                    symbol_text(symbols, segment_name)
                ),
            ));
        }
        return Ok(ResolvedNamespace::Enum(enum_id));
    }
    if let Some(enum_id) = visible_direct_enum_namespace(
        defs_by_module,
        graph,
        accessing_module,
        module_id,
        &segment_name,
        mode,
    ) {
        return Ok(ResolvedNamespace::Enum(enum_id));
    }
    Err(Diagnostic::user_error_at(
        codes::NAME_RESOLUTION,
        segment.span,
        format!("unknown namespace `{}`", symbol_text(symbols, segment_name)),
    ))
}

fn resolve_module_single(
    context: &UsingExpansionContext<'_>,
    target_surface: &ModulePublicSurface,
    target_module: ModuleId,
    name: &UsingName,
    source: PublicSource,
) -> UsingExpansion {
    let local_name = name.alias.unwrap_or(name.name);
    let local_span = name.alias_span.unwrap_or(name.name_span);
    let mut entries = Vec::new();
    if let Some(module_id) = target_surface.lookup_module(&name.name) {
        entries.push(ResolvedEntry {
            name: local_name,
            name_span: local_span,
            kind: ResolvedEntryKind::Module(module_id),
        });
    }
    if let Some(item) = target_surface.lookup_value(&name.name) {
        entries.push(ResolvedEntry {
            name: local_name,
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
    if let Some(target_defs) = context.defs_by_module.get(&target_module).copied() {
        if let Some(def_id) = target_defs.module_scope.values.get(&name.name)
            && !entries.iter().any(|entry| {
                matches!(
                    &entry.kind,
                    ResolvedEntryKind::Item(item) if item.namespace == PublicNamespace::Value
                )
            })
            && let Some(entry) = direct_item_entry(
                target_defs,
                context,
                def_id,
                local_name,
                local_span,
                source.clone(),
            )
        {
            entries.push(entry);
        }
        if let Some(def_id) = target_defs.module_scope.types.get(&name.name)
            && !entries.iter().any(|entry| {
                matches!(
                    &entry.kind,
                    ResolvedEntryKind::Item(item) if item.namespace == PublicNamespace::Type
                )
            })
            && let Some(entry) = direct_item_entry(
                target_defs,
                context,
                def_id,
                local_name,
                local_span,
                source.clone(),
            )
        {
            entries.push(entry);
        }
    }
    if entries.is_empty() {
        return UsingExpansion::Unresolved;
    }
    UsingExpansion::Resolved(entries)
}

fn resolve_current_single(
    current: &DefCollection,
    name: &UsingName,
    source: PublicSource,
) -> UsingExpansion {
    let local_name = name.alias.unwrap_or(name.name);
    let local_span = name.alias_span.unwrap_or(name.name_span);
    let mut entries = Vec::new();
    if let Some(def_id) = current.module_scope.values.get(&name.name)
        && let Some(def) = current.defs.get(def_id)
        && matches!(
            def.kind,
            DefKind::Function | DefKind::Global | DefKind::Const
        )
    {
        entries.push(ResolvedEntry {
            name: local_name,
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
    symbols: &dyn SymbolText,
    source: PublicSource,
) -> UsingExpansion {
    let Some(target_defs) = defs_by_module.get(&enum_id.module_id).copied() else {
        return UsingExpansion::Unresolved;
    };
    let Some(enum_scope) = target_defs.scopes.enum_members.get(&enum_id.def_id) else {
        return UsingExpansion::Unresolved;
    };
    match selector {
        UsingSelector::SelfName => {
            let Some(def) = target_defs.defs.get(enum_id.def_id) else {
                return UsingExpansion::Unresolved;
            };
            UsingExpansion::Resolved(vec![ResolvedEntry {
                name: def.name,
                name_span: def.span,
                kind: ResolvedEntryKind::Item(PublicItem {
                    target_module: enum_id.module_id,
                    target_def_id: enum_id.def_id,
                    namespace: PublicNamespace::Type,
                    name_span: def.span,
                    source: source.clone(),
                    parent_enum: None,
                }),
            }])
        }
        UsingSelector::Wildcard { .. } => {
            let mut entries = Vec::new();
            for (name, def_id) in enum_scope.variants.entries() {
                entries.push(ResolvedEntry {
                    name: *name,
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
        UsingSelector::Single(name) => resolve_enum_single(
            symbols,
            enum_id,
            target_defs,
            enum_scope,
            name,
            source.clone(),
        ),
        UsingSelector::Group(items) => {
            let mut entries = Vec::new();
            for item in items {
                match expand_enum_group_item(
                    symbols,
                    enum_id,
                    target_defs,
                    enum_scope,
                    item,
                    source.clone(),
                ) {
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
    symbols: &dyn SymbolText,
    enum_id: GlobalDefId,
    target_defs: &DefCollection,
    enum_scope: &nia_defs::EnumScope,
    item: &UsingGroupItem,
    source: PublicSource,
) -> UsingExpansion {
    match item {
        UsingGroupItem::Name(name) => resolve_enum_single(
            symbols,
            enum_id,
            target_defs,
            enum_scope,
            name,
            source.clone(),
        ),
        UsingGroupItem::Nested { host, .. } => {
            let span = host.first().map(|segment| segment.span).unwrap_or_default();
            UsingExpansion::HardError(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                span,
                "nested `using` group hosts are only valid under a module host",
            ))
        }
    }
}

fn resolve_enum_single(
    symbols: &dyn SymbolText,
    enum_id: GlobalDefId,
    target_defs: &DefCollection,
    enum_scope: &nia_defs::EnumScope,
    name: &UsingName,
    source: PublicSource,
) -> UsingExpansion {
    let local_name = name.alias.unwrap_or(name.name);
    let local_span = name.alias_span.unwrap_or(name.name_span);
    let Some(variant_def_id) = enum_scope.variants.get(&name.name) else {
        return UsingExpansion::HardError(Diagnostic::user_error_at(
            codes::NAME_RESOLUTION,
            name.name_span,
            format!("unknown enum variant `{}`", symbol_text(symbols, name.name)),
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
#[path = "tests.rs"]
mod tests;
