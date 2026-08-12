// SPDX-License-Identifier: GPL-3.0-or-later
use std::{borrow::Borrow, collections::HashMap};

use nia_defs::{
    DefCollection, DefKind, ModulePublicSurface, ModuleUsing, ModuleUsingScope, PathSegmentKind,
    PublicItem, PublicNamespace, PublicSource, PublicSurfaceLookup, PublicSurfaces, UsingEntry,
    UsingGroupItem, UsingName, UsingPathSegment, UsingSelector, Visibility,
};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalDefId, ModuleId};
use nia_imports::{
    ModuleGraph, ModuleRootSegment, module_declaration_visibility_allows, visibility_allows,
};
use nia_span::Span;
use nia_symbol::{
    KnownSymbolText, SymbolId, SymbolMap, SymbolText, known, symbol_text_or_unresolved,
};

mod index;
mod using_expansion;

pub use index::TypeExposureIndex;
use using_expansion::{
    ResolvedEntryKind, UsingExpansion, UsingExpansionContext, UsingLookupMode,
    collect_module_aliases, entry_already_present, expand_using, insert_into_surface,
    namespace_for, record_unresolved_using_names, root_module_for_segment,
};

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

    // Re-exports form a monotonic graph: each successful expansion only adds
    // a module or item to a surface. Iterate until no new entry appears, then
    // diagnose unresolved paths once; this preserves valid forward re-exports
    // while preventing cycles from spinning forever.
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
    // A module whose paths are not processed yet is intentionally deferred to
    // its owner, so diagnostics remain stable across incremental scheduling.
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
    let len_prelude = graph
        .std_package_root()
        .and_then(|std| graph.get(std))
        .and_then(|std| std.children.get(&known::BUILTIN).copied())
        .and_then(|builtin| surfaces.public_type(builtin, &known::LEN_TYPE));
    let mut using_scopes: HashMap<ModuleId, ModuleUsingScope> = HashMap::new();
    // Public surfaces are already closed over re-exports. Resolve each local
    // using declaration against that immutable snapshot, recording aliases in
    // source order so duplicate diagnostics point at the later declaration.
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
        if !scope.types.contains_key(&known::LEN_TYPE)
            && let Some(item) = &len_prelude
        {
            scope.types.insert(
                known::LEN_TYPE,
                UsingEntry {
                    target_module: item.target_module,
                    target_def_id: item.target_def_id,
                    namespace: PublicNamespace::Type,
                    directive_span: Span::default(),
                    name_span: item.name_span,
                    parent_enum: None,
                },
            );
        }
        using_scopes.insert(defs.module_id, scope);
    }

    PublicUsingScopes {
        using_scopes,
        diagnostics,
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
