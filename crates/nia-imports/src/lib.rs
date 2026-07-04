// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_diagnostic::{Diagnostic, codes};
pub use nia_ids::{ModuleId, Visibility};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNodeKind};
pub use nia_source::SourcePath;
use nia_source::{SourceIdentity, normalize_path};
use nia_span::Span;

pub const ENTRY_MODULE_MAP_NAME: &str = "entry";
pub const PACKAGE_MODULE_MAP_NAME: &str = "pkg";
pub const BUILTIN_MODULE_MAP_NAME: &str = "builtin";
pub const STD_MODULE_MAP_NAME: &str = "std";

pub const COMPILER_RESERVED_MODULE_ROOTS: &[&str] = &[
    ENTRY_MODULE_MAP_NAME,
    PACKAGE_MODULE_MAP_NAME,
    BUILTIN_MODULE_MAP_NAME,
];

pub fn is_compiler_reserved_module_root(name: &str) -> bool {
    COMPILER_RESERVED_MODULE_ROOTS.contains(&name)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleMap {
    entries: HashMap<String, SourcePath>,
}

impl ModuleMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: impl Into<String>, path: SourcePath) {
        let name = name.into();
        assert!(
            !is_compiler_reserved_module_root(&name),
            "`{name}` is a compiler-reserved module root"
        );
        self.entries.insert(name, path);
    }

    pub fn try_insert(&mut self, name: impl Into<String>, path: SourcePath) -> Result<(), String> {
        let name = name.into();
        if is_compiler_reserved_module_root(&name) {
            return Err(format!("`{name}` is a compiler-reserved module root"));
        }
        self.entries.insert(name, path);
        Ok(())
    }

    pub fn with_entry(&self, entry_path: SourcePath) -> Self {
        let mut map = self.clone();
        map.entries
            .insert(ENTRY_MODULE_MAP_NAME.to_string(), entry_path);
        map
    }

    pub fn with_default_std(&self, std_path: SourcePath) -> Self {
        let mut map = self.clone();
        map.entries
            .entry(STD_MODULE_MAP_NAME.to_string())
            .or_insert(std_path);
        map
    }

    pub fn get(&self, name: &str) -> Option<&SourcePath> {
        self.entries.get(name)
    }

    pub fn entries(&self) -> impl Iterator<Item = (&str, &SourcePath)> {
        self.entries
            .iter()
            .map(|(name, path)| (name.as_str(), path))
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModulePath {
    pub package: String,
    pub segments: Vec<String>,
}

impl ModulePath {
    pub fn root(package: impl Into<String>) -> Self {
        Self {
            package: package.into(),
            segments: Vec::new(),
        }
    }

    pub fn child(&self, name: impl Into<String>) -> Self {
        let mut child = self.clone();
        child.segments.push(name.into());
        child
    }

    pub fn parent(&self) -> Option<Self> {
        let mut parent = self.clone();
        parent.segments.pop()?;
        Some(parent)
    }

    pub fn is_package_root(&self) -> bool {
        self.segments.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleGraph {
    entry: ModuleId,
    modules: Vec<ModuleNode>,
    by_source_identity: HashMap<SourceIdentity, ModuleId>,
    by_module_path: HashMap<ModulePath, ModuleId>,
    package_roots: HashMap<String, ModuleId>,
    active_package_facades: HashMap<String, ModuleId>,
    diagnostics: Vec<(SourcePath, Diagnostic)>,
}

impl ModuleGraph {
    pub fn new(entry_path: SourcePath) -> Self {
        let entry = ModuleId(0);
        let entry_module_path = ModulePath::root(ENTRY_MODULE_MAP_NAME);
        let mut by_source_identity = HashMap::new();
        by_source_identity.insert(entry_path.identity(), entry);
        let mut by_module_path = HashMap::new();
        by_module_path.insert(entry_module_path.clone(), entry);
        let mut package_roots = HashMap::new();
        package_roots.insert(ENTRY_MODULE_MAP_NAME.to_string(), entry);
        Self {
            entry,
            modules: vec![ModuleNode {
                id: entry,
                path: entry_path,
                module_path: entry_module_path,
                parent: None,
                children: HashMap::new(),
                declarations: Vec::new(),
                process_used_paths: true,
                process_declared_children: true,
            }],
            by_source_identity,
            by_module_path,
            package_roots,
            active_package_facades: HashMap::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn entry(&self) -> ModuleId {
        self.entry
    }

    pub fn get(&self, id: ModuleId) -> Option<&ModuleNode> {
        self.modules.get(id.0 as usize)
    }

    pub fn module_id_for_path(&self, path: &str) -> Option<ModuleId> {
        self.module_id_for_source_identity(&SourceIdentity::new(path))
    }

    pub fn module_id_for_source_identity(&self, identity: &SourceIdentity) -> Option<ModuleId> {
        self.by_source_identity.get(identity).copied()
    }

    pub fn module_id_for_module_path(&self, path: &ModulePath) -> Option<ModuleId> {
        self.by_module_path.get(path).copied()
    }

    pub fn package_root(&self, package: &str) -> Option<ModuleId> {
        self.package_roots.get(package).copied()
    }

    pub fn mark_package_facade_active(&mut self, package: &str) -> Option<ModuleId> {
        let module_id = self.package_root(package)?;
        self.active_package_facades
            .entry(package.to_string())
            .or_insert(module_id);
        Some(module_id)
    }

    pub fn package_facade_active(&self, package: &str) -> bool {
        self.active_package_facades.contains_key(package)
    }

    pub fn current_package_root(&self, module_id: ModuleId) -> Option<ModuleId> {
        let package = &self.get(module_id)?.module_path.package;
        self.package_root(package)
    }

    pub fn modules(&self) -> impl Iterator<Item = &ModuleNode> {
        self.modules.iter()
    }

    pub fn diagnostics(&self) -> &[(SourcePath, Diagnostic)] {
        &self.diagnostics
    }

    pub fn push_diagnostic(&mut self, path: SourcePath, diagnostic: Diagnostic) {
        self.diagnostics.push((path, diagnostic));
    }

    pub fn mark_process_used_paths(&mut self, module_id: ModuleId) -> bool {
        if let Some(module) = self.modules.get_mut(module_id.0 as usize) {
            let was_enabled = module.process_used_paths;
            module.process_used_paths = true;
            !was_enabled
        } else {
            false
        }
    }

    pub fn mark_process_declared_children(&mut self, module_id: ModuleId) {
        if let Some(module) = self.modules.get_mut(module_id.0 as usize) {
            module.process_declared_children = true;
        }
    }

    pub fn intern_package_root(&mut self, name: &str, path: SourcePath) -> ModuleId {
        if let Some(id) = self.package_roots.get(name).copied() {
            return id;
        }
        let module_path = ModulePath::root(name);
        self.intern_module(path, module_path, None, false, false)
    }

    pub fn intern_declared_child(
        &mut self,
        parent_id: ModuleId,
        name: &str,
        visibility: Visibility,
        span: Span,
    ) -> Result<ModuleId, Diagnostic> {
        self.intern_declared_child_with_processing(parent_id, name, visibility, span, true, true)
    }

    pub fn intern_declared_child_with_processing(
        &mut self,
        parent_id: ModuleId,
        name: &str,
        visibility: Visibility,
        span: Span,
        process_used_paths: bool,
        process_declared_children: bool,
    ) -> Result<ModuleId, Diagnostic> {
        let Some(parent) = self.get(parent_id).cloned() else {
            return Err(Diagnostic::internal_error(
                codes::MODULE_GRAPH_LOOKUP,
                "unknown parent module id while adding module declaration",
            )
            .debug("module_id", parent_id)
            .finish());
        };
        let child_module_path = parent.module_path.child(name);
        let child_path = declared_child_source_path(&parent, name);
        let child_id = self.intern_module(
            child_path.clone(),
            child_module_path,
            Some(parent_id),
            process_used_paths,
            process_declared_children,
        );
        let parent = self.modules.get_mut(parent_id.0 as usize).ok_or_else(|| {
            Diagnostic::internal_error(
                codes::MODULE_GRAPH_RECORDING,
                "unknown parent module id while recording module declaration",
            )
            .debug("module_id", parent_id)
            .finish()
        })?;
        if let Some(existing) = parent.children.get(name).copied() {
            if existing != child_id {
                return Err(Diagnostic::internal_error(
                    codes::MODULE_GRAPH_CHILD,
                    "module child name points at a different module id",
                )
                .debug("module_id", parent_id)
                .debug("child", name)
                .finish());
            }
            return Err(Diagnostic::user_error_at(
                codes::LOAD,
                span,
                format!("duplicate module declaration `{name}`"),
            ));
        }
        parent.children.insert(name.to_string(), child_id);
        parent.declarations.push(ModuleDeclaration {
            name: name.to_string(),
            visibility,
            target: child_id,
            span,
        });
        Ok(child_id)
    }

    fn intern_module(
        &mut self,
        path: SourcePath,
        module_path: ModulePath,
        parent: Option<ModuleId>,
        process_used_paths: bool,
        process_declared_children: bool,
    ) -> ModuleId {
        if let Some(id) = self.by_module_path.get(&module_path).copied() {
            if process_used_paths {
                self.mark_process_used_paths(id);
            }
            if process_declared_children {
                self.mark_process_declared_children(id);
            }
            return id;
        }
        let identity = path.identity();
        if let Some(id) = self.by_source_identity.get(&identity).copied() {
            self.by_module_path.insert(module_path, id);
            if process_used_paths {
                self.mark_process_used_paths(id);
            }
            if process_declared_children {
                self.mark_process_declared_children(id);
            }
            return id;
        }
        let id = ModuleId(self.modules.len() as u32);
        if module_path.is_package_root() {
            self.package_roots.insert(module_path.package.clone(), id);
        }
        self.by_source_identity.insert(identity, id);
        self.by_module_path.insert(module_path.clone(), id);
        self.modules.push(ModuleNode {
            id,
            path,
            module_path,
            parent,
            children: HashMap::new(),
            declarations: Vec::new(),
            process_used_paths,
            process_declared_children,
        });
        id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleNode {
    pub id: ModuleId,
    pub path: SourcePath,
    pub module_path: ModulePath,
    pub parent: Option<ModuleId>,
    pub children: HashMap<String, ModuleId>,
    pub declarations: Vec<ModuleDeclaration>,
    pub process_used_paths: bool,
    pub process_declared_children: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleDeclaration {
    pub name: String,
    pub visibility: Visibility,
    pub target: ModuleId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedModuleDeclaration {
    pub name: String,
    pub visibility: Visibility,
    pub span: Span,
}

pub fn resolve_module_declarations_from_active_item_tree(
    diagnostics: &mut Vec<Diagnostic>,
    item_tree: &ActiveModuleItemTree,
) -> Vec<ResolvedModuleDeclaration> {
    let mut seen = HashMap::<String, Span>::new();
    let mut declarations = Vec::new();
    for item in &item_tree.items {
        let ItemTreeNodeKind::Module(module) = &item.kind else {
            continue;
        };
        if let Some(first_span) = seen.insert(module.name.clone(), item.span) {
            let _ = first_span;
            diagnostics.push(Diagnostic::user_error_at(
                codes::LOAD,
                item.span,
                format!("duplicate module declaration `{}`", module.name),
            ));
            continue;
        }
        declarations.push(ResolvedModuleDeclaration {
            name: module.name.clone(),
            visibility: item.visibility,
            span: item.span,
        });
    }
    declarations
}

pub fn add_resolved_module_declarations(
    graph: &mut ModuleGraph,
    module_id: ModuleId,
    declarations: impl IntoIterator<Item = ResolvedModuleDeclaration>,
) -> Result<(), Diagnostic> {
    for declaration in declarations {
        graph.intern_declared_child(
            module_id,
            &declaration.name,
            declaration.visibility,
            declaration.span,
        )?;
    }
    Ok(())
}

pub fn visibility_allows(
    visibility: Visibility,
    graph: &ModuleGraph,
    defining_module: ModuleId,
    accessing_module: ModuleId,
) -> bool {
    if defining_module == accessing_module {
        return true;
    }
    match visibility {
        Visibility::Public => true,
        Visibility::PublicPkg => {
            let Some(defining) = graph.get(defining_module) else {
                return false;
            };
            let Some(accessing) = graph.get(accessing_module) else {
                return false;
            };
            defining.module_path.package == accessing.module_path.package
        }
        Visibility::PublicSuper => {
            is_descendant_or_self(graph, accessing_module, defining_module)
                || graph
                    .get(defining_module)
                    .and_then(|node| node.parent)
                    .is_some_and(|parent| is_descendant_or_self(graph, accessing_module, parent))
        }
        Visibility::Private => false,
    }
}

pub fn module_declaration_visibility_allows(
    visibility: Visibility,
    graph: &ModuleGraph,
    declaring_module: ModuleId,
    accessing_module: ModuleId,
) -> bool {
    if visibility == Visibility::Private {
        return is_descendant_or_self(graph, accessing_module, declaring_module);
    }
    visibility_allows(visibility, graph, declaring_module, accessing_module)
}

fn is_descendant_or_self(graph: &ModuleGraph, module: ModuleId, ancestor: ModuleId) -> bool {
    let mut current = Some(module);
    while let Some(module_id) = current {
        if module_id == ancestor {
            return true;
        }
        current = graph.get(module_id).and_then(|node| node.parent);
    }
    false
}

pub fn declared_child_source_path(parent: &ModuleNode, child: &str) -> SourcePath {
    declared_child_source_path_for(&parent.path, &parent.module_path, child)
}

pub fn declared_child_source_path_for(
    parent_path: &SourcePath,
    parent_module_path: &ModulePath,
    child: &str,
) -> SourcePath {
    let parent_path = parent_path.as_str();
    let base = if parent_module_path.package == ENTRY_MODULE_MAP_NAME
        && parent_module_path.is_package_root()
    {
        parent_path.rsplit_once('/').map_or("", |(dir, _)| dir)
    } else {
        parent_path.strip_suffix(".nia").unwrap_or(parent_path)
    };
    let joined = if base.is_empty() {
        format!("{child}.nia")
    } else {
        format!("{base}/{child}.nia")
    };
    SourcePath::new(normalize_path(&joined))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_graph_indexes_paths_by_source_identity() {
        let mut graph = ModuleGraph::new(SourcePath::new("src/./main.nia"));

        assert_eq!(graph.module_id_for_path("src/main.nia"), Some(ModuleId(0)));
        assert_eq!(
            graph.intern_package_root("pkg", SourcePath::new("pkg/./root.nia")),
            ModuleId(1)
        );
        assert_eq!(
            graph.intern_package_root("pkg_alias", SourcePath::new("pkg/root.nia")),
            ModuleId(1)
        );
        assert_eq!(graph.module_id_for_path("pkg/root.nia"), Some(ModuleId(1)));
    }
}
