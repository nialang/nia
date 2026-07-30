// SPDX-License-Identifier: GPL-3.0-or-later
use std::{fmt, sync::Arc};

use nia_diagnostic::{Diagnostic, codes};
use nia_ids::ModuleIdAllocator;
pub use nia_ids::{DefId, GlobalDefId, ModuleId, Visibility};
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StableModuleKey(Arc<SourceIdentity>);

impl StableModuleKey {
    pub fn from_source_identity(source_identity: SourceIdentity) -> Self {
        Self(Arc::new(source_identity))
    }

    pub fn source_identity(&self) -> &SourceIdentity {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StableDefKey {
    module: StableModuleKey,
    def: DefId,
}

impl StableDefKey {
    pub fn new(module: StableModuleKey, def: DefId) -> Self {
        Self { module, def }
    }

    pub fn module(&self) -> &StableModuleKey {
        &self.module
    }

    pub fn def(&self) -> DefId {
        self.def
    }
}

#[derive(Clone)]
pub struct ModuleGraph {
    module_ids: ModuleIdAllocator,
    entry: ModuleId,
    modules: Vec<ModuleNode>,
    by_stable_key: nia_hash::FastHashMap<StableModuleKey, ModuleId>,
    by_module_path: nia_hash::FastHashMap<ModulePath, ModuleId>,
    package_roots: SymbolMap<ModuleId>,
    active_package_facades: SymbolMap<ModuleId>,
    executable_root_subtrees: Vec<ModuleId>,
    symbols: Arc<dyn SymbolText + Send + Sync>,
}

impl fmt::Debug for ModuleGraph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModuleGraph")
            .field("module_ids", &self.module_ids)
            .field("entry", &self.entry)
            .field("modules", &self.modules)
            .field("by_stable_key", &self.by_stable_key)
            .field("by_module_path", &self.by_module_path)
            .field("package_roots", &self.package_roots)
            .field("active_package_facades", &self.active_package_facades)
            .field("executable_root_subtrees", &self.executable_root_subtrees)
            .finish_non_exhaustive()
    }
}

impl PartialEq for ModuleGraph {
    fn eq(&self, other: &Self) -> bool {
        self.module_ids == other.module_ids
            && self.entry == other.entry
            && self.modules == other.modules
            && self.by_stable_key == other.by_stable_key
            && self.by_module_path == other.by_module_path
            && self.package_roots == other.package_roots
            && self.active_package_facades == other.active_package_facades
            && self.executable_root_subtrees == other.executable_root_subtrees
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleGraphSnapshot(Arc<ModuleGraph>);

impl ModuleGraphSnapshot {
    pub fn new(graph: ModuleGraph) -> Self {
        Self(Arc::new(graph))
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl From<ModuleGraph> for ModuleGraphSnapshot {
    fn from(graph: ModuleGraph) -> Self {
        Self::new(graph)
    }
}

impl std::ops::Deref for ModuleGraphSnapshot {
    type Target = ModuleGraph;

    fn deref(&self) -> &Self::Target {
        &self.0
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
        let mut module_ids = ModuleIdAllocator::new();
        let entry = module_ids.allocate();
        let entry_module_path = ModulePath::root(ENTRY_MODULE_MAP_NAME);
        let entry_stable_key = StableModuleKey::from_source_identity(entry_path.identity());
        let mut by_stable_key = nia_hash::FastHashMap::default();
        by_stable_key.insert(entry_stable_key.clone(), entry);
        let mut by_module_path = nia_hash::FastHashMap::default();
        by_module_path.insert(entry_module_path.clone(), entry);
        let mut package_roots = SymbolMap::default();
        package_roots.insert(known::ENTRY, entry);
        Self {
            module_ids,
            entry,
            modules: vec![ModuleNode {
                id: entry,
                stable_key: entry_stable_key,
                path: entry_path,
                module_path: entry_module_path,
                parent: None,
                children: SymbolMap::default(),
                declarations: Vec::new(),
                semantic_selected: true,
                process_used_paths: true,
                process_declared_children: true,
            }],
            by_stable_key,
            by_module_path,
            package_roots,
            active_package_facades: SymbolMap::default(),
            executable_root_subtrees: Vec::new(),
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
        self.modules
            .get(id.local_index() as usize)
            .filter(|module| module.id == id)
    }

    fn get_mut(&mut self, id: ModuleId) -> Option<&mut ModuleNode> {
        self.modules
            .get_mut(id.local_index() as usize)
            .filter(|module| module.id == id)
    }

    pub fn module_id_for_path(&self, path: &str) -> Option<ModuleId> {
        self.module_id_for_source_identity(&SourceIdentity::new(path))
    }

    pub fn module_id_for_source_identity(&self, identity: &SourceIdentity) -> Option<ModuleId> {
        self.module_id_for_stable_key(&StableModuleKey::from_source_identity(identity.clone()))
    }

    pub fn module_id_for_stable_key(&self, stable_key: &StableModuleKey) -> Option<ModuleId> {
        self.by_stable_key.get(stable_key).copied()
    }

    pub fn stable_key(&self, module_id: ModuleId) -> Option<&StableModuleKey> {
        Some(&self.get(module_id)?.stable_key)
    }

    pub fn stable_def_key(&self, def_id: GlobalDefId) -> Option<StableDefKey> {
        Some(StableDefKey::new(
            self.stable_key(def_id.module_id)?.clone(),
            def_id.def_id,
        ))
    }

    pub fn global_def_id_for_stable_key(&self, stable_key: &StableDefKey) -> Option<GlobalDefId> {
        Some(GlobalDefId {
            module_id: self.module_id_for_stable_key(stable_key.module())?,
            def_id: stable_key.def(),
        })
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

    pub fn mark_process_used_paths(&mut self, module_id: ModuleId) -> bool {
        if let Some(module) = self.get_mut(module_id) {
            let was_enabled = module.process_used_paths;
            module.semantic_selected = true;
            module.process_used_paths = true;
            !was_enabled
        } else {
            false
        }
    }

    pub fn mark_semantic_selected(&mut self, module_id: ModuleId) -> bool {
        if let Some(module) = self.get_mut(module_id) {
            let was_selected = module.semantic_selected;
            module.semantic_selected = true;
            !was_selected
        } else {
            false
        }
    }

    pub fn mark_process_declared_children(&mut self, module_id: ModuleId) {
        if let Some(module) = self.get_mut(module_id) {
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
        let parent = self.get_mut(parent_id).ok_or_else(|| {
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
        let stable_key = StableModuleKey::from_source_identity(path.identity());
        if let Some(id) = self.by_stable_key.get(&stable_key).copied() {
            self.by_module_path.insert(module_path, id);
            if process_used_paths {
                self.mark_process_used_paths(id);
            }
            if process_declared_children {
                self.mark_process_declared_children(id);
            }
            return id;
        }
        let id = self.module_ids.allocate();
        debug_assert_eq!(id.local_index() as usize, self.modules.len());
        if module_path.is_package_root() {
            self.package_roots.insert(module_path.package, id);
        }
        self.by_stable_key.insert(stable_key.clone(), id);
        self.by_module_path.insert(module_path.clone(), id);
        self.modules.push(ModuleNode {
            id,
            stable_key,
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
    pub stable_key: StableModuleKey,
    pub path: SourcePath,
    pub module_path: ModulePath,
    pub parent: Option<ModuleId>,
    pub children: SymbolMap<ModuleId>,
    pub declarations: Vec<ModuleDeclaration>,
    pub semantic_selected: bool,
    pub process_used_paths: bool,
    pub process_declared_children: bool,
}

pub trait ModuleGraphLookup {
    fn entry_module(&self) -> ModuleId;
    fn package_root_module(&self, package: &SymbolId) -> Option<ModuleId>;
    fn module_path(&self, module_id: ModuleId) -> Option<ModulePath>;
    fn parent_module(&self, module_id: ModuleId) -> Option<ModuleId>;
    fn child_declaration(
        &self,
        module_id: ModuleId,
        name: &SymbolId,
    ) -> Option<(ModuleId, Visibility)>;

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
    fn entry_module(&self) -> ModuleId {
        self.entry()
    }

    fn package_root_module(&self, package: &SymbolId) -> Option<ModuleId> {
        self.package_root(package)
    }

    fn module_path(&self, module_id: ModuleId) -> Option<ModulePath> {
        Some(self.get(module_id)?.module_path.clone())
    }

    fn parent_module(&self, module_id: ModuleId) -> Option<ModuleId> {
        self.get(module_id)?.parent
    }

    fn child_declaration(
        &self,
        module_id: ModuleId,
        name: &SymbolId,
    ) -> Option<(ModuleId, Visibility)> {
        let module = self.get(module_id)?;
        let target = module.children.get(name).copied()?;
        let declaration = module
            .declarations
            .iter()
            .find(|declaration| declaration.name == *name && declaration.target == target)?;
        Some((target, declaration.visibility))
    }
}

impl ModuleGraphLookup for ModuleGraphSnapshot {
    fn entry_module(&self) -> ModuleId {
        self.entry()
    }

    fn package_root_module(&self, package: &SymbolId) -> Option<ModuleId> {
        self.package_root(package)
    }

    fn module_path(&self, module_id: ModuleId) -> Option<ModulePath> {
        Some(self.get(module_id)?.module_path.clone())
    }

    fn parent_module(&self, module_id: ModuleId) -> Option<ModuleId> {
        self.get(module_id)?.parent
    }

    fn child_declaration(
        &self,
        module_id: ModuleId,
        name: &SymbolId,
    ) -> Option<(ModuleId, Visibility)> {
        let module = self.get(module_id)?;
        let target = module.children.get(name).copied()?;
        let declaration = module
            .declarations
            .iter()
            .find(|declaration| declaration.name == *name && declaration.target == target)?;
        Some((target, declaration.visibility))
    }
}

impl<T> ModuleGraphLookup for Arc<T>
where
    T: ModuleGraphLookup + ?Sized,
{
    fn entry_module(&self) -> ModuleId {
        self.as_ref().entry_module()
    }

    fn package_root_module(&self, package: &SymbolId) -> Option<ModuleId> {
        self.as_ref().package_root_module(package)
    }

    fn module_path(&self, module_id: ModuleId) -> Option<ModulePath> {
        self.as_ref().module_path(module_id)
    }

    fn parent_module(&self, module_id: ModuleId) -> Option<ModuleId> {
        self.as_ref().parent_module(module_id)
    }

    fn child_declaration(
        &self,
        module_id: ModuleId,
        name: &SymbolId,
    ) -> Option<(ModuleId, Visibility)> {
        self.as_ref().child_declaration(module_id, name)
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
    let child = resolved_module_symbol_text(symbols, child);
    let physical = declared_child_path_text(parent_path.as_str(), parent_module_path, &child);
    let logical = declared_child_path_text(
        parent_path.identity().normalized_path(),
        parent_module_path,
        &child,
    );
    SourcePath::with_identity(physical, logical)
}

fn declared_child_path_text(
    parent_path: &str,
    parent_module_path: &ModulePath,
    child: &str,
) -> String {
    let base = if parent_module_path.is_entry_package() && parent_module_path.is_package_root() {
        parent_path.rsplit_once('/').map_or("", |(dir, _)| dir)
    } else {
        parent_path.strip_suffix(".nia").unwrap_or(parent_path)
    };
    if base.is_empty() {
        format!("{child}.nia")
    } else {
        format!("{base}/{child}.nia")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_graph_indexes_paths_by_source_identity() {
        let mut graph = ModuleGraph::new(SourcePath::new("src/./main.nia"));
        let entry = graph.entry();

        assert_eq!(graph.module_id_for_path("src/main.nia"), Some(entry));
        let entry_key = graph.stable_key(entry).expect("entry stable key");
        assert_eq!(graph.module_id_for_stable_key(entry_key), Some(entry));
        let package = graph.intern_package_root(
            &module_root_symbol_from_text("pkg"),
            SourcePath::new("pkg/./root.nia"),
        );
        assert_eq!(
            graph.intern_package_root(
                &module_root_symbol_from_text("pkg_alias"),
                SourcePath::new("pkg/root.nia")
            ),
            package
        );
        assert_eq!(graph.module_id_for_path("pkg/root.nia"), Some(package));
        let package_key = graph.stable_key(package).expect("package stable key");
        assert_eq!(
            package_key.source_identity(),
            &SourcePath::new("pkg/root.nia").identity()
        );
        assert_eq!(graph.module_id_for_stable_key(package_key), Some(package));
        assert_eq!(std::mem::size_of::<ModuleGraphSnapshot>(), 8);
        assert_eq!(std::mem::size_of::<StableModuleKey>(), 8);
    }

    #[test]
    fn module_graph_rejects_foreign_handles_with_matching_local_indices() {
        let graph = ModuleGraph::new(SourcePath::new("main.nia"));
        let foreign = ModuleGraph::new(SourcePath::new("foreign.nia"));

        assert_eq!(graph.entry().local_index(), foreign.entry().local_index());
        assert_ne!(graph.entry(), foreign.entry());
        assert!(graph.get(foreign.entry()).is_none());
    }

    #[test]
    fn stable_definition_keys_remap_across_graph_owners() {
        let first = ModuleGraph::new(SourcePath::new("src/./main.nia"));
        let second = ModuleGraph::new(SourcePath::new("src/main.nia"));
        let first_local = GlobalDefId {
            module_id: first.entry(),
            def_id: DefId(42),
        };
        let second_local = GlobalDefId {
            module_id: second.entry(),
            def_id: DefId(42),
        };

        assert_ne!(first_local, second_local);
        let stable = first
            .stable_def_key(first_local)
            .expect("first stable definition key");
        assert_eq!(second.stable_def_key(second_local), Some(stable.clone()));
        assert_eq!(
            first.global_def_id_for_stable_key(&stable),
            Some(first_local)
        );
        assert_eq!(
            second.global_def_id_for_stable_key(&stable),
            Some(second_local)
        );
        assert_eq!(first.stable_def_key(second_local), None);
        assert_eq!(std::mem::size_of::<StableDefKey>(), 16);
    }

    #[test]
    fn module_graph_forks_keep_existing_handles_and_separate_new_generations() {
        let graph = ModuleGraph::new(SourcePath::new("main.nia"));
        let entry = graph.entry();
        let mut first = graph.clone();
        let mut second = graph;
        let child = module_root_symbol_from_text("child");
        let first_child = first
            .intern_declared_child(entry, &child, Visibility::Public, Span::default())
            .expect("first fork child");
        let second_child = second
            .intern_declared_child(entry, &child, Visibility::Public, Span::default())
            .expect("second fork child");

        assert_eq!(first_child.local_index(), second_child.local_index());
        assert_ne!(first_child, second_child);
        assert!(first.get(entry).is_some());
        assert!(second.get(entry).is_some());
        assert!(first.get(first_child).is_some());
        assert!(second.get(second_child).is_some());
        assert!(first.get(second_child).is_none());
        assert!(second.get(first_child).is_none());
    }

    #[test]
    fn module_graph_snapshot_rejects_handles_added_after_the_fork() {
        let mut graph = ModuleGraph::new(SourcePath::new("main.nia"));
        let snapshot = ModuleGraphSnapshot::new(graph.clone());
        let entry = graph.entry();
        let child = graph
            .intern_declared_child(
                entry,
                &module_root_symbol_from_text("child"),
                Visibility::Public,
                Span::default(),
            )
            .expect("new revision child");

        assert!(graph.get(child).is_some());
        assert!(snapshot.get(child).is_none());
    }
}
