// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::Visibility;
use nia_diagnostic::Diagnostic;
pub use nia_ids::ModuleId;
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNodeKind};
pub use nia_source::SourcePath;
use nia_span::Span;

pub const ROOT_MODULE_MAP_NAME: &str = "root";
pub const PACKAGE_MODULE_MAP_NAME: &str = "package";
pub const STD_MODULE_MAP_NAME: &str = "std";

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

    pub fn with_compiler_root(&self, root_path: SourcePath) -> Self {
        let mut map = self.clone();
        map.entries
            .insert(ROOT_MODULE_MAP_NAME.to_string(), root_path);
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
    root: ModuleId,
    modules: Vec<ModuleNode>,
    by_path: HashMap<String, ModuleId>,
    by_module_path: HashMap<ModulePath, ModuleId>,
    package_roots: HashMap<String, ModuleId>,
    diagnostics: Vec<(SourcePath, Diagnostic)>,
}

impl ModuleGraph {
    pub fn new(root_path: SourcePath) -> Self {
        let root = ModuleId(0);
        let root_module_path = ModulePath::root(ROOT_MODULE_MAP_NAME);
        let mut by_path = HashMap::new();
        by_path.insert(root_path.as_str().to_string(), root);
        let mut by_module_path = HashMap::new();
        by_module_path.insert(root_module_path.clone(), root);
        let mut package_roots = HashMap::new();
        package_roots.insert(ROOT_MODULE_MAP_NAME.to_string(), root);
        Self {
            root,
            modules: vec![ModuleNode {
                id: root,
                path: root_path,
                module_path: root_module_path,
                parent: None,
                children: HashMap::new(),
                declarations: Vec::new(),
            }],
            by_path,
            by_module_path,
            package_roots,
            diagnostics: Vec::new(),
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

    pub fn module_id_for_module_path(&self, path: &ModulePath) -> Option<ModuleId> {
        self.by_module_path.get(path).copied()
    }

    pub fn package_root(&self, package: &str) -> Option<ModuleId> {
        self.package_roots.get(package).copied()
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

    pub fn intern_package_root(&mut self, name: &str, path: SourcePath) -> ModuleId {
        if let Some(id) = self.package_roots.get(name).copied() {
            return id;
        }
        let module_path = ModulePath::root(name);
        self.intern_module(path, module_path, None)
    }

    pub fn intern_declared_child(
        &mut self,
        parent_id: ModuleId,
        name: &str,
        visibility: Visibility,
        span: Span,
    ) -> Result<ModuleId, Diagnostic> {
        let Some(parent) = self.get(parent_id).cloned() else {
            return Err(Diagnostic::internal_error(
                "I0107",
                "unknown parent module id while adding module declaration",
            )
            .debug("module_id", parent_id)
            .finish());
        };
        let child_module_path = parent.module_path.child(name);
        let child_path = child_source_path(&parent, name);
        let child_id = self.intern_module(child_path.clone(), child_module_path, Some(parent_id));
        let parent = self.modules.get_mut(parent_id.0 as usize).ok_or_else(|| {
            Diagnostic::internal_error(
                "I0108",
                "unknown parent module id while recording module declaration",
            )
            .debug("module_id", parent_id)
            .finish()
        })?;
        if let Some(existing) = parent.children.get(name).copied() {
            if existing != child_id {
                return Err(Diagnostic::internal_error(
                    "I0109",
                    "module child name points at a different module id",
                )
                .debug("module_id", parent_id)
                .debug("child", name)
                .finish());
            }
            return Err(Diagnostic::user_error_at(
                "E0102",
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
    ) -> ModuleId {
        if let Some(id) = self.by_module_path.get(&module_path).copied() {
            return id;
        }
        if let Some(id) = self.by_path.get(path.as_str()).copied() {
            self.by_module_path.insert(module_path, id);
            return id;
        }
        let id = ModuleId(self.modules.len() as u32);
        if module_path.is_package_root() {
            self.package_roots.insert(module_path.package.clone(), id);
        }
        self.by_path.insert(path.as_str().to_string(), id);
        self.by_module_path.insert(module_path.clone(), id);
        self.modules.push(ModuleNode {
            id,
            path,
            module_path,
            parent,
            children: HashMap::new(),
            declarations: Vec::new(),
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
                "E0102",
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
        Visibility::PublicPackage => {
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

fn child_source_path(parent: &ModuleNode, child: &str) -> SourcePath {
    let parent_path = parent.path.as_str();
    let base = if parent.module_path.package == ROOT_MODULE_MAP_NAME
        && parent.module_path.is_package_root()
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

pub fn normalize_path(path: &str) -> String {
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
