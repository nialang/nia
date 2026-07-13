// SPDX-License-Identifier: GPL-3.0-or-later
use std::{fmt, sync::Arc};

use nia_diagnostic::{Diagnostic, codes};
pub use nia_ids::{ModuleId, Visibility};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNodeKind};
use nia_source::SourceIdentity;
pub use nia_source::SourcePath;
use nia_span::Span;
use nia_symbol::{KnownSymbolText, SymbolId, SymbolMap, SymbolText, known, stable_hash};

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

fn module_root_symbol_from_text(name: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(name))
}

fn reserved_module_root_symbol(name: &str) -> Option<SymbolId> {
    match name {
        ENTRY_MODULE_MAP_NAME => Some(known::ENTRY),
        PACKAGE_MODULE_MAP_NAME => None,
        BUILTIN_MODULE_MAP_NAME => Some(known::BUILTIN),
        STD_MODULE_MAP_NAME => Some(known::STD),
        _ => None,
    }
}

pub fn is_entry_module_root(symbol: SymbolId) -> bool {
    symbol == known::ENTRY
}

pub fn is_std_module_root(symbol: SymbolId) -> bool {
    symbol == known::STD
}

pub fn is_builtin_module_root(symbol: SymbolId) -> bool {
    symbol == known::BUILTIN
}

pub fn module_symbol_text(symbol: SymbolId) -> String {
    fallback_module_symbol_text(symbol)
}

fn fallback_module_symbol_text(symbol: SymbolId) -> String {
    known::WELL_KNOWN
        .iter()
        .find_map(|(known, text)| (*known == symbol).then_some(*text))
        .unwrap_or("<symbol>")
        .to_string()
}

fn resolved_module_symbol_text(symbols: &dyn SymbolText, symbol: SymbolId) -> String {
    symbols
        .symbol_text(symbol)
        .map(|text| text.to_string())
        .unwrap_or_else(|| fallback_module_symbol_text(symbol))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModuleRootSegment {
    Current,
    Parent,
    PackageRelative,
    Named(SymbolId),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleMap {
    entries: SymbolMap<SourcePath>,
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
        self.entries
            .insert(module_root_symbol_from_text(&name), path);
    }

    pub fn try_insert(&mut self, name: impl Into<String>, path: SourcePath) -> Result<(), String> {
        let name = name.into();
        if is_compiler_reserved_module_root(&name) {
            return Err(format!("`{name}` is a compiler-reserved module root"));
        }
        self.entries
            .insert(module_root_symbol_from_text(&name), path);
        Ok(())
    }

    pub fn with_entry(&self, entry_path: SourcePath) -> Self {
        let mut map = self.clone();
        map.entries.insert(known::ENTRY, entry_path);
        map
    }

    pub fn with_default_std(&self, std_path: SourcePath) -> Self {
        let mut map = self.clone();
        map.entries.entry(known::STD).or_insert(std_path);
        map
    }

    pub fn get(&self, name: &str) -> Option<&SourcePath> {
        let symbol =
            reserved_module_root_symbol(name).unwrap_or_else(|| module_root_symbol_from_text(name));
        self.get_name(&symbol)
    }

    pub fn get_name(&self, name: &SymbolId) -> Option<&SourcePath> {
        self.entries.get(name)
    }

    pub fn contains_root(&self, name: SymbolId) -> bool {
        self.entries.contains_key(&name)
    }

    pub fn std_path(&self) -> Option<&SourcePath> {
        self.get_name(&known::STD)
    }

    pub fn entries(&self) -> impl Iterator<Item = (SymbolId, &SourcePath)> {
        self.entries.iter().map(|(name, path)| (*name, path))
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModulePath {
    pub package: SymbolId,
    pub segments: Vec<SymbolId>,
}

impl ModulePath {
    pub fn root(package: impl Into<String>) -> Self {
        let package = package.into();
        Self {
            package: reserved_module_root_symbol(&package)
                .unwrap_or_else(|| module_root_symbol_from_text(&package)),
            segments: Vec::new(),
        }
    }

    pub fn child(&self, name: SymbolId) -> Self {
        let mut child = self.clone();
        child.segments.push(name);
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

    pub fn is_entry_package(&self) -> bool {
        is_entry_module_root(self.package)
    }

    pub fn is_std_package(&self) -> bool {
        is_std_module_root(self.package)
    }

    pub fn is_std_start_module(&self) -> bool {
        self.is_std_package()
            && self
                .segments
                .first()
                .is_some_and(|segment| *segment == known::START)
    }
}

#[derive(Clone)]
pub struct ModuleGraph {
    entry: ModuleId,
    modules: Vec<ModuleNode>,
    by_source_identity: nia_hash::FastHashMap<SourceIdentity, ModuleId>,
    by_module_path: nia_hash::FastHashMap<ModulePath, ModuleId>,
    package_roots: SymbolMap<ModuleId>,
    active_package_facades: SymbolMap<ModuleId>,
    executable_root_subtrees: Vec<ModuleId>,
    diagnostics: Vec<(SourcePath, Diagnostic)>,
    symbols: Arc<dyn SymbolText + Send + Sync>,
}

impl fmt::Debug for ModuleGraph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModuleGraph")
            .field("entry", &self.entry)
            .field("modules", &self.modules)
            .field("by_source_identity", &self.by_source_identity)
            .field("by_module_path", &self.by_module_path)
            .field("package_roots", &self.package_roots)
            .field("active_package_facades", &self.active_package_facades)
            .field("executable_root_subtrees", &self.executable_root_subtrees)
            .field("diagnostics", &self.diagnostics)
            .finish_non_exhaustive()
    }
}

impl PartialEq for ModuleGraph {
    fn eq(&self, other: &Self) -> bool {
        self.entry == other.entry
            && self.modules == other.modules
            && self.by_source_identity == other.by_source_identity
            && self.by_module_path == other.by_module_path
            && self.package_roots == other.package_roots
            && self.active_package_facades == other.active_package_facades
            && self.executable_root_subtrees == other.executable_root_subtrees
            && self.diagnostics == other.diagnostics
    }
}

impl ModuleGraph {
    pub fn new(entry_path: SourcePath) -> Self {
        Self::with_symbol_text(entry_path, Arc::new(KnownSymbolText))
    }

    pub fn with_symbol_text(
        entry_path: SourcePath,
        symbols: Arc<dyn SymbolText + Send + Sync>,
    ) -> Self {
        let entry = ModuleId(0);
        let entry_module_path = ModulePath::root(ENTRY_MODULE_MAP_NAME);
        let mut by_source_identity = nia_hash::FastHashMap::default();
        by_source_identity.insert(entry_path.identity(), entry);
        let mut by_module_path = nia_hash::FastHashMap::default();
        by_module_path.insert(entry_module_path.clone(), entry);
        let mut package_roots = SymbolMap::default();
        package_roots.insert(known::ENTRY, entry);
        Self {
            entry,
            modules: vec![ModuleNode {
                id: entry,
                path: entry_path,
                module_path: entry_module_path,
                parent: None,
                children: SymbolMap::default(),
                declarations: Vec::new(),
                semantic_selected: true,
                process_used_paths: true,
                process_declared_children: true,
            }],
            by_source_identity,
            by_module_path,
            package_roots,
            active_package_facades: SymbolMap::default(),
            executable_root_subtrees: Vec::new(),
            diagnostics: Vec::new(),
            symbols,
        }
    }

    pub fn entry(&self) -> ModuleId {
        self.entry
    }

    pub fn mark_executable_root_subtree(&mut self, module_id: ModuleId) {
        if self.get(module_id).is_some() && !self.executable_root_subtrees.contains(&module_id) {
            self.executable_root_subtrees.push(module_id);
        }
    }

    pub fn is_executable_root_module(&self, mut module_id: ModuleId) -> bool {
        loop {
            if self.executable_root_subtrees.contains(&module_id) {
                return true;
            }
            let Some(parent) = self.get(module_id).and_then(|node| node.parent) else {
                return false;
            };
            module_id = parent;
        }
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

    pub fn package_root(&self, package: &SymbolId) -> Option<ModuleId> {
        self.package_roots.get(package).copied()
    }

    pub fn std_package_root(&self) -> Option<ModuleId> {
        self.package_root(&known::STD)
    }

    pub fn intern_std_package_root(&mut self, path: SourcePath) -> ModuleId {
        self.intern_package_root(&known::STD, path)
    }

    pub fn mark_package_facade_active(&mut self, package: &SymbolId) -> Option<ModuleId> {
        let module_id = self.package_root(package)?;
        self.active_package_facades
            .entry(*package)
            .or_insert(module_id);
        Some(module_id)
    }

    pub fn package_facade_active(&self, package: &SymbolId) -> bool {
        self.active_package_facades.contains_key(package)
    }

    pub fn std_package_facade_active(&self) -> bool {
        self.package_facade_active(&known::STD)
    }

    pub fn current_package_root(&self, module_id: ModuleId) -> Option<ModuleId> {
        let package = &self.get(module_id)?.module_path.package;
        self.package_root(package)
    }

    pub fn root_module_for_segment(
        &self,
        current_module: ModuleId,
        segment: ModuleRootSegment,
    ) -> Option<ModuleId> {
        match segment {
            ModuleRootSegment::Current => Some(current_module),
            ModuleRootSegment::Parent => self.get(current_module)?.parent,
            ModuleRootSegment::PackageRelative => self.current_package_root(current_module),
            ModuleRootSegment::Named(name) => self.root_module_for_name(current_module, name),
        }
    }

    pub fn root_module_for_name(
        &self,
        current_module: ModuleId,
        name: SymbolId,
    ) -> Option<ModuleId> {
        if is_entry_module_root(name) {
            return Some(self.entry);
        }
        self.get(current_module)
            .and_then(|node| node.children.get(&name).copied())
            .or_else(|| self.package_root(&name))
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
            module.semantic_selected = true;
            module.process_used_paths = true;
            !was_enabled
        } else {
            false
        }
    }

    pub fn mark_semantic_selected(&mut self, module_id: ModuleId) -> bool {
        if let Some(module) = self.modules.get_mut(module_id.0 as usize) {
            let was_selected = module.semantic_selected;
            module.semantic_selected = true;
            !was_selected
        } else {
            false
        }
    }

    pub fn mark_process_declared_children(&mut self, module_id: ModuleId) {
        if let Some(module) = self.modules.get_mut(module_id.0 as usize) {
            module.process_declared_children = true;
        }
    }

    pub fn intern_package_root(&mut self, name: &SymbolId, path: SourcePath) -> ModuleId {
        if let Some(id) = self.package_roots.get(name).copied() {
            return id;
        }
        let module_path = ModulePath {
            package: *name,
            segments: Vec::new(),
        };
        self.intern_module(path, module_path, None, false, false)
    }

    pub fn intern_declared_child(
        &mut self,
        parent_id: ModuleId,
        name: &SymbolId,
        visibility: Visibility,
        span: Span,
    ) -> Result<ModuleId, Diagnostic> {
        self.intern_declared_child_with_processing(parent_id, name, visibility, span, true, true)
    }

    pub fn intern_declared_child_with_processing(
        &mut self,
        parent_id: ModuleId,
        name: &SymbolId,
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
        let child_module_path = parent.module_path.child(*name);
        let child_path = self.declared_child_source_path(&parent, *name);
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
                .debug("child", self.module_symbol_text(*name))
                .finish());
            }
            return Err(Diagnostic::user_error_at(
                codes::LOAD,
                span,
                format!(
                    "duplicate module declaration `{}`",
                    self.module_symbol_text(*name)
                ),
            ));
        }
        parent.children.insert(*name, child_id);
        parent.declarations.push(ModuleDeclaration {
            name: *name,
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
            self.package_roots.insert(module_path.package, id);
        }
        self.by_source_identity.insert(identity, id);
        self.by_module_path.insert(module_path.clone(), id);
        self.modules.push(ModuleNode {
            id,
            path,
            module_path,
            parent,
            children: SymbolMap::default(),
            declarations: Vec::new(),
            semantic_selected: process_used_paths,
            process_used_paths,
            process_declared_children,
        });
        id
    }

    pub fn module_symbol_text(&self, symbol: SymbolId) -> String {
        resolved_module_symbol_text(self.symbols.as_ref(), symbol)
    }

    pub fn declared_child_source_path(&self, parent: &ModuleNode, child: SymbolId) -> SourcePath {
        declared_child_source_path_with_symbols(self.symbols.as_ref(), parent, child)
    }

    pub fn declared_child_source_path_for(
        &self,
        parent_path: &SourcePath,
        parent_module_path: &ModulePath,
        child: SymbolId,
    ) -> SourcePath {
        declared_child_source_path_for_with_symbols(
            self.symbols.as_ref(),
            parent_path,
            parent_module_path,
            child,
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleNode {
    pub id: ModuleId,
    pub path: SourcePath,
    pub module_path: ModulePath,
    pub parent: Option<ModuleId>,
    pub children: SymbolMap<ModuleId>,
    pub declarations: Vec<ModuleDeclaration>,
    pub semantic_selected: bool,
    pub process_used_paths: bool,
    pub process_declared_children: bool,
}

pub enum ModuleNodeRef<'a> {
    Borrowed(&'a ModuleNode),
    Shared(Arc<ModuleNode>),
}

impl std::ops::Deref for ModuleNodeRef<'_> {
    type Target = ModuleNode;

    fn deref(&self) -> &Self::Target {
        match self {
            ModuleNodeRef::Borrowed(node) => node,
            ModuleNodeRef::Shared(node) => node,
        }
    }
}

pub trait ModuleGraphLookup {
    fn module(&self, module_id: ModuleId) -> Option<ModuleNodeRef<'_>>;
    fn entry_module(&self) -> ModuleId;
    fn package_root_module(&self, package: &SymbolId) -> Option<ModuleId>;

    fn module_path(&self, module_id: ModuleId) -> Option<ModulePath> {
        Some(self.module(module_id)?.module_path.clone())
    }

    fn parent_module(&self, module_id: ModuleId) -> Option<ModuleId> {
        self.module(module_id)?.parent
    }

    fn child_declaration(
        &self,
        module_id: ModuleId,
        name: &SymbolId,
    ) -> Option<(ModuleId, Visibility)> {
        let module = self.module(module_id)?;
        let target = module.children.get(name).copied()?;
        let declaration = module
            .declarations
            .iter()
            .find(|declaration| declaration.name == *name && declaration.target == target)?;
        Some((target, declaration.visibility))
    }

    fn current_package_root_module(&self, module_id: ModuleId) -> Option<ModuleId> {
        let package = self.module_path(module_id)?.package;
        self.package_root_module(&package)
    }

    fn root_module_for_segment(
        &self,
        current_module: ModuleId,
        segment: ModuleRootSegment,
    ) -> Option<ModuleId> {
        match segment {
            ModuleRootSegment::Current => Some(current_module),
            ModuleRootSegment::Parent => self.parent_module(current_module),
            ModuleRootSegment::PackageRelative => self.current_package_root_module(current_module),
            ModuleRootSegment::Named(name) => {
                if is_entry_module_root(name) {
                    return Some(self.entry_module());
                }
                self.child_declaration(current_module, &name)
                    .map(|(target, _)| target)
                    .or_else(|| self.package_root_module(&name))
            }
        }
    }
}

impl ModuleGraphLookup for ModuleGraph {
    fn module(&self, module_id: ModuleId) -> Option<ModuleNodeRef<'_>> {
        self.get(module_id).map(ModuleNodeRef::Borrowed)
    }

    fn entry_module(&self) -> ModuleId {
        self.entry()
    }

    fn package_root_module(&self, package: &SymbolId) -> Option<ModuleId> {
        self.package_root(package)
    }
}

impl<T> ModuleGraphLookup for Arc<T>
where
    T: ModuleGraphLookup + ?Sized,
{
    fn module(&self, module_id: ModuleId) -> Option<ModuleNodeRef<'_>> {
        self.as_ref().module(module_id)
    }

    fn entry_module(&self) -> ModuleId {
        self.as_ref().entry_module()
    }

    fn package_root_module(&self, package: &SymbolId) -> Option<ModuleId> {
        self.as_ref().package_root_module(package)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleDeclaration {
    pub name: SymbolId,
    pub visibility: Visibility,
    pub target: ModuleId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedModuleDeclaration {
    pub name: SymbolId,
    pub visibility: Visibility,
    pub span: Span,
}

pub fn resolve_module_declarations_from_active_item_tree(
    diagnostics: &mut Vec<Diagnostic>,
    item_tree: &ActiveModuleItemTree,
) -> Vec<ResolvedModuleDeclaration> {
    resolve_module_declarations_from_active_item_tree_with_symbols(
        diagnostics,
        item_tree,
        &KnownSymbolText,
    )
}

pub fn resolve_module_declarations_from_active_item_tree_with_symbols(
    diagnostics: &mut Vec<Diagnostic>,
    item_tree: &ActiveModuleItemTree,
    symbols: &dyn SymbolText,
) -> Vec<ResolvedModuleDeclaration> {
    let mut seen = SymbolMap::<Span>::default();
    let mut declarations = Vec::new();
    for item in &item_tree.items {
        let ItemTreeNodeKind::Module(module) = &item.kind else {
            continue;
        };
        if let Some(first_span) = seen.insert(module.name, item.span) {
            let _ = first_span;
            diagnostics.push(Diagnostic::user_error_at(
                codes::LOAD,
                item.span,
                format!(
                    "duplicate module declaration `{}`",
                    resolved_module_symbol_text(symbols, module.name)
                ),
            ));
            continue;
        }
        declarations.push(ResolvedModuleDeclaration {
            name: module.name,
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
    graph: &(impl ModuleGraphLookup + ?Sized),
    defining_module: ModuleId,
    accessing_module: ModuleId,
) -> bool {
    if defining_module == accessing_module {
        return true;
    }
    match visibility {
        Visibility::Public => true,
        Visibility::PublicPkg => {
            let Some(defining) = graph.module_path(defining_module) else {
                return false;
            };
            let Some(accessing) = graph.module_path(accessing_module) else {
                return false;
            };
            defining.package == accessing.package
        }
        Visibility::PublicSuper => {
            is_descendant_or_self(graph, accessing_module, defining_module)
                || graph
                    .parent_module(defining_module)
                    .is_some_and(|parent| is_descendant_or_self(graph, accessing_module, parent))
        }
        Visibility::Private => false,
    }
}

pub fn module_declaration_visibility_allows(
    visibility: Visibility,
    graph: &(impl ModuleGraphLookup + ?Sized),
    declaring_module: ModuleId,
    accessing_module: ModuleId,
) -> bool {
    if visibility == Visibility::Private {
        return is_descendant_or_self(graph, accessing_module, declaring_module);
    }
    visibility_allows(visibility, graph, declaring_module, accessing_module)
}

fn is_descendant_or_self(
    graph: &(impl ModuleGraphLookup + ?Sized),
    module: ModuleId,
    ancestor: ModuleId,
) -> bool {
    let mut current = Some(module);
    while let Some(module_id) = current {
        if module_id == ancestor {
            return true;
        }
        current = graph.parent_module(module_id);
    }
    false
}

pub fn declared_child_source_path(parent: &ModuleNode, child: SymbolId) -> SourcePath {
    declared_child_source_path_with_symbols(&KnownSymbolText, parent, child)
}

pub fn declared_child_source_path_for(
    parent_path: &SourcePath,
    parent_module_path: &ModulePath,
    child: SymbolId,
) -> SourcePath {
    declared_child_source_path_for_with_symbols(
        &KnownSymbolText,
        parent_path,
        parent_module_path,
        child,
    )
}

pub fn declared_child_source_path_with_symbols(
    symbols: &dyn SymbolText,
    parent: &ModuleNode,
    child: SymbolId,
) -> SourcePath {
    declared_child_source_path_for_with_symbols(symbols, &parent.path, &parent.module_path, child)
}

pub fn declared_child_source_path_for_with_symbols(
    symbols: &dyn SymbolText,
    parent_path: &SourcePath,
    parent_module_path: &ModulePath,
    child: SymbolId,
) -> SourcePath {
    let parent_path = parent_path.as_str();
    let child = resolved_module_symbol_text(symbols, child);
    let base = if parent_module_path.is_entry_package() && parent_module_path.is_package_root() {
        parent_path.rsplit_once('/').map_or("", |(dir, _)| dir)
    } else {
        parent_path.strip_suffix(".nia").unwrap_or(parent_path)
    };
    let joined = if base.is_empty() {
        format!("{child}.nia")
    } else {
        format!("{base}/{child}.nia")
    };
    SourcePath::from_normalized_unchecked(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_graph_indexes_paths_by_source_identity() {
        let mut graph = ModuleGraph::new(SourcePath::new("src/./main.nia"));

        assert_eq!(graph.module_id_for_path("src/main.nia"), Some(ModuleId(0)));
        assert_eq!(
            graph.intern_package_root(
                &module_root_symbol_from_text("pkg"),
                SourcePath::new("pkg/./root.nia")
            ),
            ModuleId(1)
        );
        assert_eq!(
            graph.intern_package_root(
                &module_root_symbol_from_text("pkg_alias"),
                SourcePath::new("pkg/root.nia")
            ),
            ModuleId(1)
        );
        assert_eq!(graph.module_id_for_path("pkg/root.nia"), Some(ModuleId(1)));
    }
}
