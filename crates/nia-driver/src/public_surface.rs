// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_ast::{UsingSelector, Visibility};
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
                match expand_using(defs_by_module, defs, using, imports, &surfaces) {
                    UsingExpansion::Resolved(entries) => {
                        let surface = surfaces
                            .get(defs.module_id)
                            .cloned()
                            .unwrap_or_else(|| ModulePublicSurface::new(defs.module_id));
                        let mut surface = surface;
                        for entry in entries {
                            if entry_already_present(&surface, &entry.name, entry.item.namespace) {
                                continue;
                            }
                            iteration_changed = true;
                            insert_into_surface(&mut surface, &entry.name, entry.item);
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
            match expand_using(defs_by_module, defs, using, imports, &surfaces) {
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
            let entries = match expand_using(defs_by_module, defs, using, imports, &surfaces) {
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
                let using_entry = UsingEntry {
                    target_module: entry.item.target_module,
                    target_def_id: entry.item.target_def_id,
                    namespace: entry.item.namespace,
                    directive_span: using.span,
                    name_span: entry.name_span,
                    parent_enum: entry.item.parent_enum,
                };
                let table = match entry.item.namespace {
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
        using_scopes.insert(defs.module_id, scope);
    }

    (surfaces, using_scopes, diagnostics)
}

fn namespace_for(kind: DefKind) -> Option<PublicNamespace> {
    match kind {
        DefKind::Function | DefKind::Global => Some(PublicNamespace::Value),
        DefKind::Struct | DefKind::Enum | DefKind::TypeAlias => Some(PublicNamespace::Type),
        DefKind::Import | DefKind::Method | DefKind::StructField | DefKind::EnumVariant => None,
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

struct ResolvedEntry {
    name: String,
    name_span: Span,
    item: PublicItem,
}

enum UsingExpansion {
    Resolved(Vec<ResolvedEntry>),
    Unresolved,
    HardError(Diagnostic),
}

/// Resolved descriptor of a `using` host (the part before the final `::`).
enum HostBinding {
    /// `using mod::...` — host names an imported module alias.
    Module { target_module: ModuleId },
    /// `using Enum::...` or `using mod::Enum::...` — host names an enum.
    Enum { enum_id: GlobalDefId },
}

fn resolve_host(
    defs_by_module: &[DefCollection],
    current: &DefCollection,
    using: &ModuleUsing,
    imports: &ImportAliasMap,
) -> Result<HostBinding, Diagnostic> {
    let first = &using.host[0];
    match using.host.len() {
        1 => {
            // Try module alias first.
            if let Some(import) = imports.get(current.module_id, &first.name) {
                return Ok(HostBinding::Module {
                    target_module: import.target,
                });
            }
            // Then try a local enum.
            if let Some(def_id) = current.module_scope.types.get(&first.name)
                && let Some(def) = current.defs.get(def_id)
                && def.kind == DefKind::Enum
            {
                return Ok(HostBinding::Enum {
                    enum_id: GlobalDefId {
                        module_id: current.module_id,
                        def_id,
                    },
                });
            }
            Err(Diagnostic::error(
                first.span,
                format!(
                    "`using {}::...` requires `{0}` to be an imported module alias or a local enum",
                    first.name
                ),
            ))
        }
        2 => {
            let second = &using.host[1];
            let Some(import) = imports.get(current.module_id, &first.name) else {
                return Err(Diagnostic::error(
                    first.span,
                    format!(
                        "`using {}::...` requires `{0}` to be an imported module alias",
                        first.name
                    ),
                ));
            };
            // Look up the second segment as a pub enum in target module.
            let Some(target_defs) = defs_by_module
                .iter()
                .find(|defs| defs.module_id == import.target)
            else {
                return Err(Diagnostic::error(
                    second.span,
                    "import alias refers to an unloaded module",
                ));
            };
            let Some(def_id) = target_defs.module_scope.types.get(&second.name) else {
                return Err(Diagnostic::error(
                    second.span,
                    format!("unknown type `{}::{}`", first.name, second.name),
                ));
            };
            let Some(def) = target_defs.defs.get(def_id) else {
                return Err(Diagnostic::error(second.span, "enum definition not found"));
            };
            if def.kind != DefKind::Enum {
                return Err(Diagnostic::error(
                    second.span,
                    format!(
                        "`using {}::{}::...` requires `{}::{}` to be an enum",
                        first.name, second.name, first.name, second.name
                    ),
                ));
            }
            if def.visibility != Visibility::Public {
                return Err(Diagnostic::error(
                    second.span,
                    format!("enum `{}::{}` is private", first.name, second.name),
                ));
            }
            Ok(HostBinding::Enum {
                enum_id: GlobalDefId {
                    module_id: import.target,
                    def_id,
                },
            })
        }
        _ => Err(Diagnostic::error(
            using.host[2].span,
            "`using` host accepts at most two segments",
        )),
    }
}

fn expand_using(
    defs_by_module: &[DefCollection],
    current: &DefCollection,
    using: &ModuleUsing,
    imports: &ImportAliasMap,
    surfaces: &PublicSurfaces,
) -> UsingExpansion {
    let host = match resolve_host(defs_by_module, current, using, imports) {
        Ok(host) => host,
        Err(diag) => return UsingExpansion::HardError(diag),
    };
    let visible = using.visibility == Visibility::Public;
    let source = || PublicSource::PubUsing {
        directive_span: using.span,
    };
    match host {
        HostBinding::Module { target_module } => {
            expand_module_host(target_module, &using.selector, surfaces, source)
        }
        HostBinding::Enum { enum_id } => {
            expand_enum_host(enum_id, defs_by_module, &using.selector, visible, source)
        }
    }
}

fn expand_module_host<F>(
    target_module: ModuleId,
    selector: &UsingSelector,
    surfaces: &PublicSurfaces,
    source: F,
) -> UsingExpansion
where
    F: Fn() -> PublicSource,
{
    let Some(target_surface) = surfaces.get(target_module) else {
        return UsingExpansion::Unresolved;
    };
    match selector {
        UsingSelector::Wildcard { .. } => {
            let mut entries = Vec::new();
            for (name, public_item) in target_surface
                .values
                .iter()
                .chain(target_surface.types.iter())
            {
                if !matches!(public_item.source, PublicSource::Direct) {
                    continue;
                }
                entries.push(ResolvedEntry {
                    name: name.clone(),
                    name_span: public_item_name_span(public_item),
                    item: PublicItem {
                        target_module: public_item.target_module,
                        target_def_id: public_item.target_def_id,
                        namespace: public_item.namespace,
                        source: source(),
                        parent_enum: public_item.parent_enum,
                    },
                });
            }
            UsingExpansion::Resolved(entries)
        }
        UsingSelector::Single(name) => resolve_module_single(target_surface, name, &source),
        UsingSelector::Group(names) => {
            let mut entries = Vec::new();
            let mut any_unresolved = false;
            let mut seen: HashSet<(String, PublicNamespace)> = HashSet::new();
            for name in names {
                match resolve_module_single(target_surface, name, &source) {
                    UsingExpansion::Resolved(mut sub) => {
                        for entry in sub.drain(..) {
                            if seen.insert((entry.name.clone(), entry.item.namespace)) {
                                entries.push(entry);
                            }
                        }
                    }
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

fn resolve_module_single<F>(
    target_surface: &ModulePublicSurface,
    name: &nia_ast::UsingName,
    source: &F,
) -> UsingExpansion
where
    F: Fn() -> PublicSource,
{
    let local_name = name.alias.clone().unwrap_or_else(|| name.name.clone());
    let local_span = name.alias_span.unwrap_or(name.name_span);
    let mut entries = Vec::new();
    if let Some(item) = target_surface.lookup_value(&name.name) {
        entries.push(ResolvedEntry {
            name: local_name.clone(),
            name_span: local_span,
            item: PublicItem {
                target_module: item.target_module,
                target_def_id: item.target_def_id,
                namespace: PublicNamespace::Value,
                source: source(),
                parent_enum: item.parent_enum,
            },
        });
    }
    if let Some(item) = target_surface.lookup_type(&name.name) {
        entries.push(ResolvedEntry {
            name: local_name,
            name_span: local_span,
            item: PublicItem {
                target_module: item.target_module,
                target_def_id: item.target_def_id,
                namespace: PublicNamespace::Type,
                source: source(),
                parent_enum: item.parent_enum,
            },
        });
    }
    if entries.is_empty() {
        return UsingExpansion::Unresolved;
    }
    UsingExpansion::Resolved(entries)
}

fn expand_enum_host<F>(
    enum_id: GlobalDefId,
    defs_by_module: &[DefCollection],
    selector: &UsingSelector,
    _visible: bool,
    source: F,
) -> UsingExpansion
where
    F: Fn() -> PublicSource,
{
    let Some(target_defs) = defs_by_module
        .iter()
        .find(|defs| defs.module_id == enum_id.module_id)
    else {
        return UsingExpansion::Unresolved;
    };
    let Some(enum_scope) = target_defs.scopes.enum_members.get(&enum_id.def_id) else {
        return UsingExpansion::Unresolved;
    };
    match selector {
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
                    item: PublicItem {
                        target_module: enum_id.module_id,
                        target_def_id: def_id,
                        namespace: PublicNamespace::Value,
                        source: source(),
                        parent_enum: Some(enum_id),
                    },
                });
            }
            UsingExpansion::Resolved(entries)
        }
        UsingSelector::Single(name) => {
            resolve_enum_single(enum_id, target_defs, enum_scope, name, &source)
        }
        UsingSelector::Group(names) => {
            let mut entries = Vec::new();
            let mut seen: HashSet<String> = HashSet::new();
            for name in names {
                match resolve_enum_single(enum_id, target_defs, enum_scope, name, &source) {
                    UsingExpansion::Resolved(mut sub) => {
                        for entry in sub.drain(..) {
                            if seen.insert(entry.name.clone()) {
                                entries.push(entry);
                            }
                        }
                    }
                    UsingExpansion::Unresolved => return UsingExpansion::Unresolved,
                    UsingExpansion::HardError(diag) => return UsingExpansion::HardError(diag),
                }
            }
            UsingExpansion::Resolved(entries)
        }
    }
}

fn resolve_enum_single<F>(
    enum_id: GlobalDefId,
    target_defs: &DefCollection,
    enum_scope: &nia_defs::EnumScope,
    name: &nia_ast::UsingName,
    source: &F,
) -> UsingExpansion
where
    F: Fn() -> PublicSource,
{
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
        item: PublicItem {
            target_module: enum_id.module_id,
            target_def_id: variant_def_id,
            namespace: PublicNamespace::Value,
            source: source(),
            parent_enum: Some(enum_id),
        },
    }])
}

fn public_item_name_span(_item: &PublicItem) -> Span {
    Span::default()
}
