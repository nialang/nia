//! Import aliases retain the same access routes as ordinary selector expansion.

use super::*;

pub(crate) fn collect_module_aliases(
    defs_by_module: &HashMap<ModuleId, &DefCollection>,
    current: &DefCollection,
    graph: &ModuleGraph,
    surfaces: &PublicSurfaces,
    symbols: &dyn SymbolText,
) -> ModuleUsingScope {
    let mut scope = ModuleUsingScope::default();
    for using in &current.module_usings {
        let context = UsingExpansionContext {
            defs_by_module,
            graph,
            accessing_module: current.module_id,
            surfaces,
            symbols,
            mode: UsingLookupMode::Visible,
            imported_types: Some(&scope.types),
            imported_scopes: Some(&scope),
            lexical_namespaces: None,
        };
        let expansion = expand_using(&context, current, using, &scope.modules);
        for entry in expansion.entries {
            match entry.kind {
                ResolvedEntryKind::Module(module) => {
                    if let std::collections::hash_map::Entry::Vacant(target) =
                        scope.modules.entry(entry.name)
                    {
                        target.insert(module);
                        scope.module_scopes.insert(entry.name, entry.scope);
                    }
                }
                ResolvedEntryKind::Item(item) => {
                    let key = (item.namespace, entry.name);
                    let table = match item.namespace {
                        PublicNamespace::Type => &mut scope.types,
                        PublicNamespace::Value => &mut scope.values,
                    };
                    if let std::collections::hash_map::Entry::Vacant(target) =
                        table.entry(entry.name)
                    {
                        target.insert(UsingEntry {
                            target_module: item.target_module,
                            target_def_id: item.target_def_id,
                            namespace: item.namespace,
                            directive_span: using.span,
                            name_span: entry.name_span,
                            parent_enum: item.parent_enum,
                        });
                        scope.item_scopes.insert(key, entry.scope);
                    }
                }
            }
        }
    }
    scope
}
