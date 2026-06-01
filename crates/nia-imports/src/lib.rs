// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{ImportPath, ImportPathKind, ItemKind, Module};
use nia_diagnostic::Diagnostic;
pub use nia_ids::ModuleId;
pub use nia_source::SourcePath;
use nia_span::Span;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleMap {
    entries: HashMap<String, SourcePath>,
}

impl ModuleMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: impl Into<String>, path: SourcePath) {
        self.entries.insert(name.into(), path);
    }

    pub fn get(&self, name: &str) -> Option<&SourcePath> {
        self.entries.get(name)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleGraph {
    root: ModuleId,
    modules: Vec<ModuleNode>,
    by_path: HashMap<String, ModuleId>,
}

impl ModuleGraph {
    pub fn new(root_path: SourcePath) -> Self {
        let root = ModuleId(0);
        let mut by_path = HashMap::new();
        by_path.insert(root_path.as_str().to_string(), root);
        Self {
            root,
            modules: vec![ModuleNode {
                id: root,
                path: root_path,
                imports: Vec::new(),
            }],
            by_path,
        }
    }

    pub fn root(&self) -> ModuleId {
        self.root
    }

    pub fn get(&self, id: ModuleId) -> Option<&ModuleNode> {
        self.modules.get(id.0 as usize)
    }

    pub fn module_id_for_path(&self, path: &str) -> Option<ModuleId> {
        self.by_path.get(path).copied()
    }

    pub fn modules(&self) -> impl Iterator<Item = &ModuleNode> {
        self.modules.iter()
    }

    fn intern_path(&mut self, path: SourcePath) -> ModuleId {
        if let Some(id) = self.by_path.get(path.as_str()) {
            return *id;
        }
        let id = ModuleId(self.modules.len() as u32);
        self.by_path.insert(path.as_str().to_string(), id);
        self.modules.push(ModuleNode {
            id,
            path,
            imports: Vec::new(),
        });
        id
    }

    fn add_import(&mut self, from: ModuleId, import: ImportEdge) -> Result<(), Diagnostic> {
        let Some(module) = self.modules.get_mut(from.0 as usize) else {
            return Err(Diagnostic::error(
                Span::default(),
                format!("internal error: unknown source module id {from:?}"),
            ));
        };
        module.imports.push(import);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleNode {
    pub id: ModuleId,
    pub path: SourcePath,
    pub imports: Vec<ImportEdge>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportEdge {
    pub alias: String,
    pub path: SourcePath,
    pub target: ModuleId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedImport {
    pub alias: String,
    pub path: SourcePath,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportCollection {
    pub graph: ModuleGraph,
    pub aliases: ImportAliasMap,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportAliasMap {
    modules: HashMap<ModuleId, HashMap<String, ImportAlias>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportAlias {
    pub alias: String,
    pub target: ModuleId,
    pub span: Span,
}

impl ImportAliasMap {
    pub fn get(&self, module: ModuleId, alias: &str) -> Option<&ImportAlias> {
        self.modules.get(&module)?.get(alias)
    }

    pub fn module_aliases(&self, module: ModuleId) -> Option<&HashMap<String, ImportAlias>> {
        self.modules.get(&module)
    }

    pub fn modules(&self) -> impl Iterator<Item = (ModuleId, &HashMap<String, ImportAlias>)> {
        self.modules
            .iter()
            .map(|(module, aliases)| (*module, aliases))
    }
}

pub fn collect_root_imports(root_path: SourcePath, root_module: &Module) -> ImportCollection {
    collect_root_imports_with_map(root_path, root_module, &ModuleMap::default())
}

pub fn collect_root_imports_with_map(
    root_path: SourcePath,
    root_module: &Module,
    module_map: &ModuleMap,
) -> ImportCollection {
    let mut graph = ModuleGraph::new(root_path.clone());
    let mut diagnostics = Vec::new();
    let root = graph.root();
    collect_module_imports(
        &mut graph,
        &mut diagnostics,
        root,
        &root_path,
        root_module,
        module_map,
    );
    let aliases = collect_import_aliases(&graph);
    ImportCollection {
        graph,
        aliases,
        diagnostics,
    }
}

pub fn collect_import_aliases(graph: &ModuleGraph) -> ImportAliasMap {
    let mut aliases = ImportAliasMap::default();
    for module in graph.modules() {
        for import in &module.imports {
            aliases.modules.entry(module.id).or_default().insert(
                import.alias.clone(),
                ImportAlias {
                    alias: import.alias.clone(),
                    target: import.target,
                    span: import.span,
                },
            );
        }
    }
    aliases
}

pub fn collect_module_imports(
    graph: &mut ModuleGraph,
    diagnostics: &mut Vec<Diagnostic>,
    module_id: ModuleId,
    module_path: &SourcePath,
    module: &Module,
    module_map: &ModuleMap,
) {
    // Validate the source node before interning any target paths. Otherwise a
    // bad caller could leave unreachable modules in the graph while losing the
    // edge that explains why they were discovered.
    if graph.get(module_id).is_none() {
        diagnostics.push(Diagnostic::error(
            Span::default(),
            format!("internal error: unknown module id {module_id:?} while collecting imports"),
        ));
        return;
    }

    for import in resolve_module_imports(diagnostics, module_path, module, module_map) {
        let target = graph.intern_path(import.path.clone());
        if let Err(diagnostic) = graph.add_import(
            module_id,
            ImportEdge {
                alias: import.alias,
                path: import.path,
                target,
                span: import.span,
            },
        ) {
            diagnostics.push(diagnostic);
            return;
        }
    }
}

pub fn resolve_module_imports(
    diagnostics: &mut Vec<Diagnostic>,
    module_path: &SourcePath,
    module: &Module,
    module_map: &ModuleMap,
) -> Vec<ResolvedImport> {
    let mut aliases = HashMap::<String, Span>::new();
    let mut imports = Vec::new();
    for item in &module.items {
        let ItemKind::Import(import) = &item.kind else {
            continue;
        };
        let alias = import
            .alias
            .clone()
            .unwrap_or_else(|| import_default_alias(&import.path));
        if let Some(first_span) = aliases.get(&alias).copied() {
            let _ = first_span;
            diagnostics.push(Diagnostic::error(
                item.span,
                format!("duplicate import alias: `{alias}`"),
            ));
            continue;
        }
        let Some(path) = resolve_import_path(module_path, &import.path, module_map) else {
            diagnostics.push(Diagnostic::error(
                item.span,
                format!(
                    "unknown module mapping `{}`; configure it with `-M {0}=path`",
                    import.path.segments[0]
                ),
            ));
            continue;
        };
        aliases.insert(alias.clone(), item.span);
        imports.push(ResolvedImport {
            alias,
            path,
            span: item.span,
        });
    }
    imports
}

pub fn add_resolved_imports(
    graph: &mut ModuleGraph,
    module_id: ModuleId,
    imports: impl IntoIterator<Item = ResolvedImport>,
) -> Result<(), Diagnostic> {
    // The query loader only calls this with ids read from the same graph. Keep
    // that as an explicit boundary error so graph corruption is reported before
    // unreachable target modules can be interned.
    if graph.get(module_id).is_none() {
        return Err(Diagnostic::error(
            Span::default(),
            format!(
                "internal error: unknown module id {module_id:?} while adding resolved imports"
            ),
        ));
    }

    for import in imports {
        let target = graph.intern_path(import.path.clone());
        graph.add_import(
            module_id,
            ImportEdge {
                alias: import.alias,
                path: import.path,
                target,
                span: import.span,
            },
        )?;
    }
    Ok(())
}

pub fn resolve_import_path(
    module_path: &SourcePath,
    import_path: &ImportPath,
    module_map: &ModuleMap,
) -> Option<SourcePath> {
    match import_path.kind {
        ImportPathKind::Root => {
            let head = import_path.segments.first()?;
            let mapped = module_map.get(head)?;
            if import_path.segments.len() == 1 {
                return Some(SourcePath::new(normalize_path(mapped.as_str())));
            }
            let base_dir = mapped
                .as_str()
                .strip_suffix(".nia")
                .unwrap_or(mapped.as_str())
                .to_string();
            let tail = import_path.segments[1..].join("/");
            let joined = if base_dir.is_empty() {
                format!("{tail}.nia")
            } else {
                format!("{base_dir}/{tail}.nia")
            };
            Some(SourcePath::new(normalize_path(&joined)))
        }
        ImportPathKind::Relative { parents } => {
            let module_dir = module_path
                .as_str()
                .rsplit_once('/')
                .map_or("", |(dir, _)| dir);
            let mut prefix = String::from(module_dir);
            for _ in 0..parents {
                prefix.push_str("/..");
            }
            let tail = import_path.segments.join("/");
            let joined = if prefix.is_empty() {
                format!("{tail}.nia")
            } else {
                format!("{prefix}/{tail}.nia")
            };
            Some(SourcePath::new(normalize_path(&joined)))
        }
    }
}

pub fn import_default_alias(path: &ImportPath) -> String {
    path.segments
        .last()
        .cloned()
        .unwrap_or_else(|| "_".to_string())
}

fn normalize_path(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    let normalized = parts.join("/");
    if absolute {
        format!("/{normalized}")
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_parser::parse_module;

    #[test]
    fn collects_root_import_edges_without_loading_modules() {
        let (module, errors) = parse_module(
            r#"
import .math;
import .lib.io as io;
fn main() {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let collection = collect_root_imports(SourcePath::new("src/main.nia"), &module);
        assert!(
            collection.diagnostics.is_empty(),
            "{:?}",
            collection.diagnostics
        );
        let root = collection
            .graph
            .get(collection.graph.root())
            .expect("root module");
        assert_eq!(root.imports.len(), 2);
        assert_eq!(root.imports[0].alias, "math");
        assert_eq!(root.imports[0].path.as_str(), "src/math.nia");
        assert_eq!(root.imports[1].alias, "io");
        assert_eq!(root.imports[1].path.as_str(), "src/lib/io.nia");
    }

    #[test]
    fn reports_duplicate_import_aliases_per_module() {
        let (module, errors) = parse_module(
            r#"
import .a.math as math;
import .b.math as math;
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let collection = collect_root_imports(SourcePath::new("main.nia"), &module);
        assert_eq!(collection.diagnostics.len(), 1);
        assert!(
            collection.diagnostics[0]
                .message
                .contains("duplicate import alias")
        );
    }

    #[test]
    fn normalizes_relative_import_paths() {
        use nia_ast::{ImportPath, ImportPathKind};
        let base = SourcePath::new("src/app/main.nia");
        let map = ModuleMap::default();
        assert_eq!(
            resolve_import_path(
                &base,
                &ImportPath {
                    kind: ImportPathKind::Relative { parents: 1 },
                    segments: vec!["lib".into(), "math".into()],
                },
                &map,
            )
            .map(|path| path.as_str().to_string()),
            Some("src/lib/math.nia".to_string())
        );
        assert_eq!(
            resolve_import_path(
                &base,
                &ImportPath {
                    kind: ImportPathKind::Relative { parents: 0 },
                    segments: vec!["util".into()],
                },
                &map,
            )
            .map(|path| path.as_str().to_string()),
            Some("src/app/util.nia".to_string())
        );
    }

    #[test]
    fn root_import_requires_module_map() {
        let (module, errors) = parse_module(r#"import math;"#);
        assert!(errors.is_empty(), "{errors:?}");
        let collection = collect_root_imports(SourcePath::new("src/main.nia"), &module);
        assert_eq!(collection.diagnostics.len(), 1);
        assert!(
            collection.diagnostics[0]
                .message
                .contains("unknown module mapping")
        );
    }

    #[test]
    fn root_import_uses_module_map_when_provided() {
        let (module, errors) = parse_module(
            r#"
import math;
import math.ops;
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let mut map = ModuleMap::default();
        map.insert("math", SourcePath::new("/usr/share/nia/math.nia"));
        let collection =
            collect_root_imports_with_map(SourcePath::new("src/main.nia"), &module, &map);
        assert!(
            collection.diagnostics.is_empty(),
            "{:?}",
            collection.diagnostics
        );
        let root = collection
            .graph
            .get(collection.graph.root())
            .expect("root module");
        assert_eq!(root.imports[0].path.as_str(), "/usr/share/nia/math.nia");
        assert_eq!(root.imports[1].path.as_str(), "/usr/share/nia/math/ops.nia");
    }

    #[test]
    fn collect_module_imports_reports_invalid_source_module_id() {
        let (module, errors) = parse_module("import .math;");
        assert!(errors.is_empty(), "{errors:?}");
        let mut graph = ModuleGraph::new(SourcePath::new("main.nia"));
        let mut diagnostics = Vec::new();

        collect_module_imports(
            &mut graph,
            &mut diagnostics,
            ModuleId(99),
            &SourcePath::new("main.nia"),
            &module,
            &ModuleMap::default(),
        );

        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("unknown module id"));
        assert_eq!(graph.modules().count(), 1);
        assert!(
            graph
                .get(graph.root())
                .expect("root module")
                .imports
                .is_empty()
        );
    }

    #[test]
    fn add_resolved_imports_reports_invalid_source_module_id() {
        let mut graph = ModuleGraph::new(SourcePath::new("main.nia"));

        let err = add_resolved_imports(
            &mut graph,
            ModuleId(99),
            [ResolvedImport {
                alias: "math".to_string(),
                path: SourcePath::new("math.nia"),
                span: Span::default(),
            }],
        )
        .expect_err("unknown source module id should be reported");

        assert!(err.message.contains("unknown module id"));
        assert_eq!(graph.modules().count(), 1);
    }
}
