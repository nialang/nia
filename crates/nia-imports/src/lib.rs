// SPDX-License-Identifier: GPL-3.0-or-later
//! Module maps, stable identities, and import-graph construction utilities.
//!
//! A [`ModuleMap`] binds package names to source roots; the `entry`, `root`,
//! `builtin`, `std`, and private `runtime` roots are reserved for the
//! compiler. The [`ModuleGraph`] is built from those roots and normalizes
//! `using` paths relative to the current module, its parent, the current
//! package root, or a named package or child declaration. Graph construction
//! reports duplicate module declarations; resolving and checking the names a
//! `using` selects belongs to `nia-public-surface`.
//!
//! Stable module and definition keys derive from source identities rather
//! than session handles. The graph retains each module's owning
//! [`SourcePath`], which semantic consumers use when a diagnostic relates a
//! use site to a declaration in another module.

use std::{fmt, sync::Arc};

use nia_diagnostic::{Diagnostic, codes};
use nia_ice::Ice;
use nia_ids::ModuleIdAllocator;
pub use nia_ids::{DefId, GlobalDefId, ModuleId, Visibility};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNodeKind};
pub use nia_source::{SourceId, SourcePath};
use nia_source::{SourceIdentity, SourceStoreId, SourceTable};
use nia_span::Span;
use nia_symbol::{
    KnownSymbolText, SymbolId, SymbolMap, SymbolText, known, stable_hash, symbol_identity_key,
};

/// Reserved module-map name for the root of the current module tree.
pub const ROOT_MODULE_MAP_NAME: &str = "root";
/// Reserved module-map name for compiler builtins.
pub const BUILTIN_MODULE_MAP_NAME: &str = "builtin";
/// Reserved module-map name for the standard library.
pub const STD_MODULE_MAP_NAME: &str = "std";
/// Dependency name through which the runtime reaches the program it starts.
/// It is visible only inside the runtime's module tree.
pub const PROGRAM_MODULE_MAP_NAME: &str = "program";

/// Names that cannot be inserted as ordinary package roots.
pub const COMPILER_RESERVED_MODULE_ROOTS: &[&str] =
    &[ROOT_MODULE_MAP_NAME, BUILTIN_MODULE_MAP_NAME];

/// Failure produced while mutating a module graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleGraphError {
    /// A source-level error that should be rendered for the user.
    Diagnostic(Diagnostic),
    /// A violated compiler invariant or exhausted identity space.
    Internal(Ice),
}

impl From<Diagnostic> for ModuleGraphError {
    fn from(diagnostic: Diagnostic) -> Self {
        Self::Diagnostic(diagnostic)
    }
}

impl From<Ice> for ModuleGraphError {
    fn from(ice: Ice) -> Self {
        Self::Internal(ice)
    }
}

/// Reports whether text names a compiler-reserved module root.
pub fn is_compiler_reserved_module_root(name: &str) -> bool {
    COMPILER_RESERVED_MODULE_ROOTS.contains(&name)
}

fn module_root_symbol_from_text(name: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(name))
}

fn reserved_module_root_symbol(name: &str) -> Option<SymbolId> {
    match name {
        ROOT_MODULE_MAP_NAME => None,
        BUILTIN_MODULE_MAP_NAME => Some(known::BUILTIN),
        STD_MODULE_MAP_NAME => Some(known::STD),
        _ => None,
    }
}

/// Reports whether a symbol denotes the standard-library root.
pub fn is_std_module_root(symbol: SymbolId) -> bool {
    symbol == known::STD
}

/// Reports whether a symbol denotes the builtin root.
pub fn is_builtin_module_root(symbol: SymbolId) -> bool {
    symbol == known::BUILTIN
}

/// Returns known text for a module root, with a stable identity fallback.
pub fn module_symbol_text(symbol: SymbolId) -> String {
    fallback_module_symbol_text(symbol)
}

fn fallback_module_symbol_text(symbol: SymbolId) -> String {
    known::WELL_KNOWN
        .iter()
        .find_map(|(known, text)| (*known == symbol).then_some(*text))
        .map(str::to_owned)
        .unwrap_or_else(|| symbol_identity_key(symbol))
}

fn resolved_module_symbol_text(symbols: &dyn SymbolText, symbol: SymbolId) -> Arc<str> {
    symbols
        .symbol_text(symbol)
        .unwrap_or_else(|| fallback_module_symbol_text(symbol).into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Root selection mode for resolving an import path.
pub enum ModuleRootSegment {
    /// Resolve from the current module.
    Current,
    /// Resolve from the current module's parent.
    Parent,
    /// Resolve from the root of the current module tree.
    Root,
    /// Resolve a named package or child root.
    Named(SymbolId),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
/// Mapping from source package names to module root paths.
pub struct ModuleMap {
    entries: SymbolMap<SourcePath>,
    scopes: std::collections::BTreeMap<String, SymbolMap<SourcePath>>,
}

impl ModuleMap {
    /// Creates an empty module map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a non-reserved root and reports reserved-name errors.
    pub fn insert(&mut self, name: impl Into<String>, path: SourcePath) -> Result<(), String> {
        self.try_insert(name, path)
    }

    /// Inserts a non-reserved root and reports reserved-name errors.
    pub fn try_insert(&mut self, name: impl Into<String>, path: SourcePath) -> Result<(), String> {
        let name = name.into();
        if is_compiler_reserved_module_root(&name) {
            return Err(format!("`{name}` is a compiler-reserved module root"));
        }
        self.entries
            .insert(module_root_symbol_from_text(&name), path);
        Ok(())
    }

    /// Returns a copy with a default standard-library root.
    pub fn with_default_std(&self, std_path: SourcePath) -> Self {
        let mut map = self.clone();
        map.entries.entry(known::STD).or_insert(std_path);
        map
    }

    /// Looks up a root by source text, including reserved aliases.
    pub fn get(&self, name: &str) -> Option<&SourcePath> {
        let symbol =
            reserved_module_root_symbol(name).unwrap_or_else(|| module_root_symbol_from_text(name));
        self.get_name(&symbol)
    }

    /// Looks up a root by stable symbol identity.
    pub fn get_name(&self, name: &SymbolId) -> Option<&SourcePath> {
        self.entries.get(name)
    }

    /// Reports whether a root identity is present.
    pub fn contains_root(&self, name: SymbolId) -> bool {
        self.entries.contains_key(&name)
    }

    /// Returns the configured standard-library path, if any.
    pub fn std_path(&self) -> Option<&SourcePath> {
        self.get_name(&known::STD)
    }

    /// Iterates root identities and source paths.
    pub fn entries(&self) -> impl Iterator<Item = (SymbolId, &SourcePath)> {
        self.entries.iter().map(|(name, path)| (*name, path))
    }

    /// Reports whether this map has no roots.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.scopes.is_empty()
    }

    /// Defines the dependency namespace for a logical package source directory.
    pub fn add_scope(&mut self, directory: &SourceIdentity) {
        self.scopes
            .entry(directory.normalized_path().trim_end_matches('/').to_owned())
            .or_default();
    }

    /// Binds the runtime's `program` dependency to the program's entry file.
    ///
    /// The runtime reaches the program it starts the way a package reaches a
    /// dependency: through a name visible only inside its own directory, so
    /// no other module can name it. The binding resolves to the program
    /// tree's root, which the graph already owns.
    pub fn with_runtime_program(
        &self,
        runtime_directory: &SourceIdentity,
        program: SourcePath,
    ) -> Self {
        let mut map = self.clone();
        map.scopes
            .entry(
                runtime_directory
                    .normalized_path()
                    .trim_end_matches('/')
                    .to_owned(),
            )
            .or_default()
            .insert(known::PROGRAM, program);
        map
    }

    /// Adds a dependency alias visible only inside one logical package directory.
    pub fn insert_scoped(
        &mut self,
        directory: &SourceIdentity,
        name: &str,
        path: SourcePath,
    ) -> Result<(), String> {
        if is_compiler_reserved_module_root(name) {
            return Err(format!("`{name}` is a compiler-reserved module root"));
        }
        let scope = self
            .scopes
            .entry(directory.normalized_path().trim_end_matches('/').to_owned())
            .or_default();
        let name = module_root_symbol_from_text(name);
        if scope.get(&name).is_some_and(|existing| existing != &path) {
            return Err(
                "dependency alias resolves to multiple packages in the same package namespace"
                    .to_owned(),
            );
        }
        scope.insert(name, path);
        Ok(())
    }

    fn source_scope(&self, source: &SourcePath) -> Option<&SymbolMap<SourcePath>> {
        let identity = source.identity_ref().normalized_path();
        self.scopes
            .iter()
            .filter(|(prefix, _)| {
                identity
                    .strip_prefix(prefix.as_str())
                    .is_some_and(|suffix| suffix.starts_with('/'))
            })
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(_, scope)| scope)
    }

    /// Reports whether a source belongs to an explicitly isolated package namespace.
    pub fn has_scope(&self, source: &SourcePath) -> bool {
        self.source_scope(source).is_some()
    }

    /// Returns the names visible to one source module, preserving compiler-owned roots.
    pub fn for_source(&self, source: &SourcePath) -> Self {
        let Some(scope) = self.source_scope(source) else {
            return self.clone();
        };
        let mut entries = self.entries.clone();
        entries.extend(scope.iter().map(|(name, path)| (*name, path.clone())));
        Self {
            entries,
            scopes: Default::default(),
        }
    }

    /// Resolves a source-local alias to its stable package instance and source root.
    pub fn binding_for_source(
        &self,
        source: &SourcePath,
        name: SymbolId,
    ) -> Option<(SymbolId, &SourcePath)> {
        if let Some(path) = self.source_scope(source).and_then(|scope| scope.get(&name)) {
            // The runtime's `program` is the program tree itself, not a
            // separate instance of the same files.
            if name == known::PROGRAM {
                return Some((known::PROGRAM_TREE, path));
            }
            let key = format!(
                "nia.package.instance:{}",
                path.identity_ref().normalized_path()
            );
            return Some((module_root_symbol_from_text(&key), path));
        }
        if self.has_scope(source) && name != known::STD {
            return None;
        }
        self.entries.get(&name).map(|path| (name, path))
    }

    /// Iterates package namespaces, including empty namespaces that isolate a package.
    pub fn scopes(&self) -> impl Iterator<Item = (&str, &SymbolMap<SourcePath>)> {
        self.scopes
            .iter()
            .map(|(prefix, entries)| (prefix.as_str(), entries))
    }

    /// Iterates all roots for source registration and eager package-root selection.
    pub fn all_entries(&self) -> impl Iterator<Item = (SymbolId, &SourcePath)> {
        self.entries().chain(self.scopes.values().flat_map(|scope| {
            scope.values().map(|path| {
                (
                    module_root_symbol_from_text(&format!(
                        "nia.package.instance:{}",
                        path.identity_ref().normalized_path()
                    )),
                    path,
                )
            })
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Stable package and module path identity.
pub struct ModulePath {
    /// Package root identity.
    pub package: SymbolId,
    /// Child module segments below the package root.
    pub segments: Vec<SymbolId>,
}

impl ModulePath {
    /// Creates a package-root path from source text.
    pub fn root(package: impl Into<String>) -> Self {
        let package = package.into();
        Self {
            package: reserved_module_root_symbol(&package)
                .unwrap_or_else(|| module_root_symbol_from_text(&package)),
            segments: Vec::new(),
        }
    }

    /// Appends one child module segment.
    pub fn child(&self, name: SymbolId) -> Self {
        let mut child = self.clone();
        child.segments.push(name);
        child
    }

    /// Removes one child segment, returning `None` at the package root.
    pub fn parent(&self) -> Option<Self> {
        let mut parent = self.clone();
        parent.segments.pop()?;
        Some(parent)
    }

    /// Reports whether this path is exactly a package root.
    pub fn is_package_root(&self) -> bool {
        self.segments.is_empty()
    }

    /// Reports whether this path belongs to the program's own module tree.
    pub fn is_program_tree(&self) -> bool {
        self.package == known::PROGRAM_TREE
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Stable module identity backed by a source identity.
pub struct StableModuleKey(Arc<SourceIdentity>);

impl StableModuleKey {
    /// Creates a stable key from a source identity.
    pub fn from_source_identity(source_identity: SourceIdentity) -> Self {
        Self(Arc::new(source_identity))
    }

    /// Returns the source identity carried by this key.
    pub fn source_identity(&self) -> &SourceIdentity {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Stable definition identity combining a module key and local definition id.
pub struct StableDefKey {
    module: StableModuleKey,
    def: DefId,
}

impl StableDefKey {
    /// Creates a stable definition key.
    pub fn new(module: StableModuleKey, def: DefId) -> Self {
        Self { module, def }
    }

    /// Returns the owning stable module key.
    pub fn module(&self) -> &StableModuleKey {
        &self.module
    }

    /// Returns the local definition id.
    pub fn def(&self) -> DefId {
        self.def
    }
}

#[derive(Clone)]
/// Mutable module graph keyed by stable source and module-path identities.
pub struct ModuleGraph {
    module_ids: ModuleIdAllocator,
    sources: SourceTable,
    entry: ModuleId,
    modules: Vec<ModuleNode>,
    by_source_id: nia_hash::FastHashMap<SourceId, ModuleId>,
    by_stable_key: nia_hash::FastHashMap<StableModuleKey, ModuleId>,
    by_module_path: nia_hash::FastHashMap<ModulePath, ModuleId>,
    package_roots: SymbolMap<ModuleId>,
    module_package_aliases: nia_hash::FastHashMap<ModuleId, SymbolMap<ModuleId>>,
    active_package_facades: SymbolMap<ModuleId>,
    executable_root_subtrees: Vec<ModuleId>,
    symbols: Arc<dyn SymbolText + Send + Sync>,
}

impl fmt::Debug for ModuleGraph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModuleGraph")
            .field("module_ids", &self.module_ids)
            .field("source_store_id", &self.sources.id())
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
            && self.by_source_id == other.by_source_id
            && self.by_stable_key == other.by_stable_key
            && self.by_module_path == other.by_module_path
            && self.package_roots == other.package_roots
            && self.module_package_aliases == other.module_package_aliases
            && self.active_package_facades == other.active_package_facades
            && self.executable_root_subtrees == other.executable_root_subtrees
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Cheaply clonable, pointer-comparable snapshot of a module graph.
pub struct ModuleGraphSnapshot(Arc<ModuleGraph>);

struct DeclaredChildSpec {
    parent_id: ModuleId,
    name: SymbolId,
    visibility: Visibility,
    span: Span,
    child_path: SourcePath,
    process_used_paths: bool,
    process_declared_children: bool,
    module_path: ModulePath,
}

impl ModuleGraphSnapshot {
    /// Wraps a graph in shared snapshot storage.
    pub fn new(graph: ModuleGraph) -> Self {
        Self(Arc::new(graph))
    }

    /// Reports whether two snapshots share the same graph allocation.
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
    /// Creates a graph with the supplied entry source and known symbols.
    pub fn new(entry_path: SourcePath) -> nia_ice::IceResult<Self> {
        Self::with_symbol_text(entry_path, Arc::new(KnownSymbolText))
    }

    /// Creates a graph with an explicit symbol-text provider.
    pub fn with_symbol_text(
        entry_path: SourcePath,
        symbols: Arc<dyn SymbolText + Send + Sync>,
    ) -> nia_ice::IceResult<Self> {
        Self::with_source_table(entry_path, symbols, SourceTable::new())
    }

    /// Creates a graph bound to an explicit source/path interner.
    pub fn with_source_table(
        entry_path: SourcePath,
        symbols: Arc<dyn SymbolText + Send + Sync>,
        sources: SourceTable,
    ) -> nia_ice::IceResult<Self> {
        let module_ids = ModuleIdAllocator::new()?;
        let entry = module_ids.allocate()?;
        let entry_source_id = sources
            .id_for_path(&entry_path)
            .map_err(|error| Ice::new(error.to_string()))?;
        let entry_module_path = ModulePath {
            package: known::PROGRAM_TREE,
            segments: Vec::new(),
        };
        let entry_stable_key = StableModuleKey::from_source_identity(entry_path.identity());
        let mut by_stable_key = nia_hash::FastHashMap::default();
        by_stable_key.insert(entry_stable_key.clone(), entry);
        let mut by_module_path = nia_hash::FastHashMap::default();
        by_module_path.insert(entry_module_path.clone(), entry);
        let mut package_roots = SymbolMap::default();
        package_roots.insert(known::PROGRAM_TREE, entry);
        let mut by_source_id = nia_hash::FastHashMap::default();
        by_source_id.insert(entry_source_id, entry);
        Ok(Self {
            module_ids,
            sources,
            entry,
            modules: vec![ModuleNode {
                id: entry,
                stable_key: entry_stable_key,
                source_id: entry_source_id,
                path: entry_path,
                module_path: entry_module_path,
                parent: None,
                children: SymbolMap::default(),
                declarations: Vec::new(),
                provider_dependencies: Vec::new(),
                trait_impl_dependencies: Vec::new(),
                semantic_selected: true,
                process_used_paths: true,
                process_declared_children: true,
            }],
            by_source_id,
            by_stable_key,
            by_module_path,
            package_roots,
            module_package_aliases: Default::default(),
            active_package_facades: SymbolMap::default(),
            executable_root_subtrees: Vec::new(),
            symbols,
        })
    }

    /// Returns the entry module id.
    pub fn entry(&self) -> ModuleId {
        self.entry
    }

    /// Returns the source-store owner accepted by this graph.
    pub fn source_store_id(&self) -> SourceStoreId {
        self.sources.id()
    }

    /// Resolves an owner-qualified source handle to its module.
    pub fn module_id_for_source_id(&self, source_id: SourceId) -> Option<ModuleId> {
        self.by_source_id.get(&source_id).copied()
    }

    /// Marks a module and all descendants as executable roots.
    pub fn mark_executable_root_subtree(&mut self, module_id: ModuleId) {
        if self.get(module_id).is_some() && !self.executable_root_subtrees.contains(&module_id) {
            self.executable_root_subtrees.push(module_id);
        }
    }

    /// Reports whether a module is inside an executable root subtree.
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

    /// Looks up a module node by its allocated id.
    pub fn get(&self, id: ModuleId) -> Option<&ModuleNode> {
        self.modules
            .get(usize::try_from(id.local_index()).ok()?)
            .filter(|module| module.id == id)
    }

    fn get_mut(&mut self, id: ModuleId) -> Option<&mut ModuleNode> {
        self.modules
            .get_mut(usize::try_from(id.local_index()).ok()?)
            .filter(|module| module.id == id)
    }

    /// Resolves a source path to its module id.
    pub fn module_id_for_path(&self, path: &str) -> Option<ModuleId> {
        self.module_id_for_source_identity(&SourceIdentity::new(path))
    }

    /// Resolves a source identity to its module id.
    pub fn module_id_for_source_identity(&self, identity: &SourceIdentity) -> Option<ModuleId> {
        self.module_id_for_stable_key(&StableModuleKey::from_source_identity(identity.clone()))
    }

    /// Resolves a stable module key to its allocated id.
    pub fn module_id_for_stable_key(&self, stable_key: &StableModuleKey) -> Option<ModuleId> {
        self.by_stable_key.get(stable_key).copied()
    }

    /// Returns the stable key for a module id.
    pub fn stable_key(&self, module_id: ModuleId) -> Option<&StableModuleKey> {
        Some(&self.get(module_id)?.stable_key)
    }

    /// Converts a global definition id to its stable key.
    pub fn stable_def_key(&self, def_id: GlobalDefId) -> Option<StableDefKey> {
        Some(StableDefKey::new(
            self.stable_key(def_id.module_id)?.clone(),
            def_id.def_id,
        ))
    }

    /// Converts a stable definition key back to a current global id.
    pub fn global_def_id_for_stable_key(&self, stable_key: &StableDefKey) -> Option<GlobalDefId> {
        Some(GlobalDefId {
            module_id: self.module_id_for_stable_key(stable_key.module())?,
            def_id: stable_key.def(),
        })
    }

    /// Resolves a module path to its allocated id.
    pub fn module_id_for_module_path(&self, path: &ModulePath) -> Option<ModuleId> {
        self.by_module_path.get(path).copied()
    }

    /// Looks up a package root by symbol identity.
    pub fn package_root(&self, package: &SymbolId) -> Option<ModuleId> {
        self.package_roots.get(package).copied()
    }

    /// Returns the standard-library package root, if interned.
    pub fn std_package_root(&self) -> Option<ModuleId> {
        self.package_root(&known::STD)
    }

    /// Interns and returns the standard-library package root.
    pub fn intern_std_package_root(&mut self, path: SourcePath) -> nia_ice::IceResult<ModuleId> {
        self.intern_package_root(&known::STD, path)
    }

    /// Interns the injected runtime's package root.
    ///
    /// Its tree identity is not an identifier, so user source cannot name it;
    /// the runtime reaches its own modules through `root::`.
    pub fn intern_runtime_package_root(
        &mut self,
        path: SourcePath,
    ) -> nia_ice::IceResult<ModuleId> {
        self.intern_package_root(&known::RUNTIME_TREE, path)
    }

    /// Returns the injected runtime's package root, if a runtime is injected.
    pub fn runtime_package_root(&self) -> Option<ModuleId> {
        self.package_root(&known::RUNTIME_TREE)
    }

    /// Marks an interned package facade active and returns its root.
    pub fn mark_package_facade_active(&mut self, package: &SymbolId) -> Option<ModuleId> {
        let module_id = self.package_root(package)?;
        self.active_package_facades
            .entry(*package)
            .or_insert(module_id);
        Some(module_id)
    }

    /// Reports whether a package facade has been marked active.
    pub fn package_facade_active(&self, package: &SymbolId) -> bool {
        self.active_package_facades.contains_key(package)
    }

    /// Returns the package root containing a module.
    pub fn current_package_root(&self, module_id: ModuleId) -> Option<ModuleId> {
        let package = &self.get(module_id)?.module_path.package;
        self.package_root(package)
    }

    /// Resolves a root selector relative to a current module.
    pub fn root_module_for_segment(
        &self,
        current_module: ModuleId,
        segment: ModuleRootSegment,
    ) -> Option<ModuleId> {
        match segment {
            ModuleRootSegment::Current => Some(current_module),
            ModuleRootSegment::Parent => self.get(current_module)?.parent,
            ModuleRootSegment::Root => self.current_package_root(current_module),
            ModuleRootSegment::Named(name) => self.root_module_for_name(current_module, name),
        }
    }

    /// Resolves a named root from a current module or package map.
    pub fn root_module_for_name(
        &self,
        current_module: ModuleId,
        name: SymbolId,
    ) -> Option<ModuleId> {
        self.get(current_module)
            .and_then(|node| node.children.get(&name).copied())
            .or_else(|| {
                if let Some(aliases) = self.module_package_aliases.get(&current_module) {
                    aliases.get(&name).copied().or_else(|| {
                        is_std_module_root(name)
                            .then(|| self.package_root(&name))
                            .flatten()
                    })
                } else {
                    self.package_root(&name)
                }
            })
    }

    /// Isolates a module's named package roots from the global package-instance registry.
    pub fn isolate_package_roots(&mut self, module: ModuleId) {
        self.module_package_aliases.entry(module).or_default();
    }

    /// Binds one source-local alias to an interned package instance.
    pub fn bind_package_alias(&mut self, module: ModuleId, alias: SymbolId, target: ModuleId) {
        self.module_package_aliases
            .entry(module)
            .or_default()
            .insert(alias, target);
    }

    /// Iterates all module nodes in allocation order.
    pub fn modules(&self) -> impl Iterator<Item = &ModuleNode> {
        self.modules.iter()
    }

    /// Enables used-path processing and reports whether it changed state.
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

    /// Marks a module semantically selected and reports whether it changed state.
    pub fn mark_semantic_selected(&mut self, module_id: ModuleId) -> bool {
        if let Some(module) = self.get_mut(module_id) {
            let was_selected = module.semantic_selected;
            module.semantic_selected = true;
            !was_selected
        } else {
            false
        }
    }

    /// Records a provider module selected for semantic analysis of `module_id`.
    pub fn add_provider_dependency(
        &mut self,
        module_id: ModuleId,
        provider_module: ModuleId,
    ) -> bool {
        let Some(module) = self.get_mut(module_id) else {
            return false;
        };
        match module.provider_dependencies.binary_search(&provider_module) {
            Ok(_) => false,
            Err(index) => {
                module.provider_dependencies.insert(index, provider_module);
                true
            }
        }
    }

    /// Records a trait witness without importing the provider's extensions.
    pub fn add_trait_impl_dependency(
        &mut self,
        module_id: ModuleId,
        provider_module: ModuleId,
    ) -> bool {
        let Some(module) = self.get_mut(module_id) else {
            return false;
        };
        match module
            .trait_impl_dependencies
            .binary_search(&provider_module)
        {
            Ok(_) => false,
            Err(index) => {
                module
                    .trait_impl_dependencies
                    .insert(index, provider_module);
                true
            }
        }
    }

    /// Enables processing of declared child modules.
    pub fn mark_process_declared_children(&mut self, module_id: ModuleId) {
        if let Some(module) = self.get_mut(module_id) {
            module.process_declared_children = true;
        }
    }

    /// Interns a package root unless it already exists.
    pub fn intern_package_root(
        &mut self,
        name: &SymbolId,
        path: SourcePath,
    ) -> nia_ice::IceResult<ModuleId> {
        if let Some(id) = self.package_roots.get(name).copied() {
            return Ok(id);
        }
        let module_path = ModulePath {
            package: *name,
            segments: Vec::new(),
        };
        let id = self.intern_module(path, module_path, None, false, false)?;
        // A stable source path can be shared by package aliases. Keep every
        // requested package identity resolvable even when interning reuses the
        // existing module node.
        self.package_roots.insert(*name, id);
        Ok(id)
    }

    /// Interns a declared child with default processing flags.
    pub fn intern_declared_child(
        &mut self,
        parent_id: ModuleId,
        name: &SymbolId,
        visibility: Visibility,
        span: Span,
    ) -> Result<ModuleId, ModuleGraphError> {
        self.intern_declared_child_with_processing(parent_id, name, visibility, span, true, true)
    }

    /// Interns a declared child while explicitly selecting graph processing flags.
    pub fn intern_declared_child_with_processing(
        &mut self,
        parent_id: ModuleId,
        name: &SymbolId,
        visibility: Visibility,
        span: Span,
        process_used_paths: bool,
        process_declared_children: bool,
    ) -> Result<ModuleId, ModuleGraphError> {
        let Some(parent) = self.get(parent_id).cloned() else {
            return Err(Ice::new(format!(
                "unknown parent module {parent_id:?} while adding a module declaration"
            ))
            .into());
        };
        let child_module_path = parent.module_path.child(*name);
        let (_, child_path) = self.intern_declared_child_source_path(&parent, *name)?;
        self.intern_declared_child_with_source_path_and_processing(DeclaredChildSpec {
            parent_id,
            name: *name,
            visibility,
            span,
            child_path,
            process_used_paths,
            process_declared_children,
            module_path: child_module_path,
        })
    }

    /// Interns a declared child using an explicit source path.
    ///
    /// Runtime resources may live outside a package's source directory while
    /// retaining the package/module namespace used by source-level imports.
    /// Callers must provide the canonical [`SourcePath`] selected by the
    /// loader; no filesystem path is inferred from the parent in this form.
    pub fn intern_declared_child_with_source_path(
        &mut self,
        parent_id: ModuleId,
        name: &SymbolId,
        visibility: Visibility,
        span: Span,
        child_path: SourcePath,
    ) -> Result<ModuleId, ModuleGraphError> {
        let Some(parent) = self.get(parent_id).cloned() else {
            return Err(Ice::new(format!(
                "unknown parent module {parent_id:?} while adding a module declaration"
            ))
            .into());
        };
        let child_module_path = parent.module_path.child(*name);
        self.intern_declared_child_with_source_path_and_processing(DeclaredChildSpec {
            parent_id,
            name: *name,
            visibility,
            span,
            child_path,
            process_used_paths: true,
            process_declared_children: true,
            module_path: child_module_path,
        })
    }

    fn intern_declared_child_with_source_path_and_processing(
        &mut self,
        child: DeclaredChildSpec,
    ) -> Result<ModuleId, ModuleGraphError> {
        let DeclaredChildSpec {
            parent_id,
            name,
            visibility,
            span,
            child_path,
            process_used_paths,
            process_declared_children,
            module_path,
        } = child;
        let child_id = self.intern_module(
            child_path.clone(),
            module_path,
            Some(parent_id),
            process_used_paths,
            process_declared_children,
        )?;
        let parent = self.get_mut(parent_id).ok_or_else(|| {
            Ice::new(format!(
                "unknown parent module {parent_id:?} while recording a module declaration"
            ))
        })?;
        if let Some(existing) = parent.children.get(&name).copied() {
            if existing != child_id {
                return Err(Ice::new(format!(
                    "child `{}` of module {parent_id:?} points at inconsistent module identities",
                    self.module_symbol_text(name)
                ))
                .into());
            }
            return Err(Diagnostic::user_error_at(
                codes::LOAD,
                span,
                format!(
                    "duplicate module declaration `{}`",
                    self.module_symbol_text(name)
                ),
            )
            .into());
        }
        parent.children.insert(name, child_id);
        parent.declarations.push(ModuleDeclaration {
            name,
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
    ) -> nia_ice::IceResult<ModuleId> {
        let source_id = self
            .sources
            .id_for_path(&path)
            .map_err(|error| Ice::new(error.to_string()))?;
        if let Some(id) = self.by_module_path.get(&module_path).copied() {
            if process_used_paths {
                self.mark_process_used_paths(id);
            }
            if process_declared_children {
                self.mark_process_declared_children(id);
            }
            return Ok(id);
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
            return Ok(id);
        }
        let id = self.module_ids.allocate()?;
        if id.local_index() as usize != self.modules.len() {
            return Err(nia_ice::Ice::new(
                "module ID allocation order does not match the module graph",
            ));
        }
        if module_path.is_package_root() {
            self.package_roots.insert(module_path.package, id);
        }
        self.by_stable_key.insert(stable_key.clone(), id);
        self.by_source_id.insert(source_id, id);
        self.by_module_path.insert(module_path.clone(), id);
        self.modules.push(ModuleNode {
            id,
            stable_key,
            source_id,
            path,
            module_path,
            parent,
            children: SymbolMap::default(),
            declarations: Vec::new(),
            provider_dependencies: Vec::new(),
            trait_impl_dependencies: Vec::new(),
            semantic_selected: process_used_paths,
            process_used_paths,
            process_declared_children,
        });
        Ok(id)
    }

    /// Resolves a module symbol through the graph's symbol provider.
    pub fn module_symbol_text(&self, symbol: SymbolId) -> String {
        self.symbols
            .symbol_text(symbol)
            .map(|text| text.to_string())
            .unwrap_or_else(|| fallback_module_symbol_text(symbol))
    }

    /// Computes the source path for a declared child module.
    pub fn declared_child_source_path(&self, parent: &ModuleNode, child: SymbolId) -> SourcePath {
        declared_child_source_path_with_symbols(self.symbols.as_ref(), parent, child)
    }

    /// Interns and returns a declared child's owner-qualified source handle.
    pub fn intern_declared_child_source_path(
        &self,
        parent: &ModuleNode,
        child: SymbolId,
    ) -> nia_ice::IceResult<(SourceId, SourcePath)> {
        let sibling = parent.module_path.is_package_root();
        self.intern_child_source_path(parent.source_id, child, sibling)
    }

    /// Interns and returns a child below a non-root source file.
    pub fn intern_nested_child_source_path(
        &self,
        parent: SourceId,
        child: SymbolId,
    ) -> nia_ice::IceResult<(SourceId, SourcePath)> {
        self.intern_child_source_path(parent, child, false)
    }

    fn intern_child_source_path(
        &self,
        parent: SourceId,
        child: SymbolId,
        sibling: bool,
    ) -> nia_ice::IceResult<(SourceId, SourcePath)> {
        let child = resolved_module_symbol_text(self.symbols.as_ref(), child);
        let source_id = self
            .sources
            .id_for_child_path(parent, &child, sibling)
            .map_err(|error| Ice::new(error.to_string()))?;
        self.sources.path_for_id(source_id).map_or_else(
            || Err(Ice::new("source interner lost a derived child path")),
            |path| Ok((source_id, path.as_ref().clone())),
        )
    }

    /// Computes a declared child path from explicit parent identities.
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
/// One module node and its declaration/selection metadata.
pub struct ModuleNode {
    /// Allocated module identity.
    pub id: ModuleId,
    /// Stable source identity key.
    pub stable_key: StableModuleKey,
    /// Owner-qualified source/path interner handle.
    pub source_id: SourceId,
    /// Source path used to load the module.
    pub path: SourcePath,
    /// Package and child-segment identity.
    pub module_path: ModulePath,
    /// Parent module, if this is not a package root.
    pub parent: Option<ModuleId>,
    /// Declared child modules keyed by name.
    pub children: SymbolMap<ModuleId>,
    /// Source declarations exported by this module.
    pub declarations: Vec<ModuleDeclaration>,
    /// Provider modules selected through this module's semantic demands.
    pub provider_dependencies: Vec<ModuleId>,
    /// Trait witnesses selected independently of extension import scope.
    pub trait_impl_dependencies: Vec<ModuleId>,
    /// Whether semantic analysis selected this module.
    pub semantic_selected: bool,
    /// Whether `using` paths should be processed.
    pub process_used_paths: bool,
    /// Whether declared child modules should be processed.
    pub process_declared_children: bool,
}

/// Read-only lookup contract implemented by module graph products.
pub trait ModuleGraphLookup {
    /// Returns the entry module id.
    fn entry_module(&self) -> ModuleId;
    /// Returns a package root module by identity.
    fn package_root_module(&self, package: &SymbolId) -> Option<ModuleId>;
    /// Returns a module's stable package path.
    fn module_path(&self, module_id: ModuleId) -> Option<ModulePath>;
    /// Returns the source file that owns a module.
    fn source_path(&self, module_id: ModuleId) -> Option<SourcePath>;
    /// Returns a module's parent.
    fn parent_module(&self, module_id: ModuleId) -> Option<ModuleId>;
    /// Resolves a declared child and its visibility.
    fn child_declaration(
        &self,
        module_id: ModuleId,
        name: &SymbolId,
    ) -> Option<(ModuleId, Visibility)>;
    /// Returns provider modules selected for this module's semantic demands.
    fn provider_dependencies(&self, module_id: ModuleId, trait_impls_only: bool) -> Vec<ModuleId>;

    /// Returns the package root containing a module.
    fn current_package_root_module(&self, module_id: ModuleId) -> Option<ModuleId> {
        let package = self.module_path(module_id)?.package;
        self.package_root_module(&package)
    }

    /// Resolves a root selector relative to a module.
    fn root_module_for_segment(
        &self,
        current_module: ModuleId,
        segment: ModuleRootSegment,
    ) -> Option<ModuleId> {
        match segment {
            ModuleRootSegment::Current => Some(current_module),
            ModuleRootSegment::Parent => self.parent_module(current_module),
            ModuleRootSegment::Root => self.current_package_root_module(current_module),
            ModuleRootSegment::Named(name) => self
                .child_declaration(current_module, &name)
                .map(|(target, _)| target)
                .or_else(|| self.package_root_module(&name)),
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

    fn source_path(&self, module_id: ModuleId) -> Option<SourcePath> {
        Some(self.get(module_id)?.path.clone())
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

    fn provider_dependencies(&self, module_id: ModuleId, trait_impls_only: bool) -> Vec<ModuleId> {
        self.get(module_id)
            .map(|module| {
                if trait_impls_only {
                    module.trait_impl_dependencies.clone()
                } else {
                    module.provider_dependencies.clone()
                }
            })
            .unwrap_or_default()
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

    fn source_path(&self, module_id: ModuleId) -> Option<SourcePath> {
        Some(self.get(module_id)?.path.clone())
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

    fn provider_dependencies(&self, module_id: ModuleId, trait_impls_only: bool) -> Vec<ModuleId> {
        self.get(module_id)
            .map(|module| {
                if trait_impls_only {
                    module.trait_impl_dependencies.clone()
                } else {
                    module.provider_dependencies.clone()
                }
            })
            .unwrap_or_default()
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

    fn source_path(&self, module_id: ModuleId) -> Option<SourcePath> {
        self.as_ref().source_path(module_id)
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

    fn provider_dependencies(&self, module_id: ModuleId, trait_impls_only: bool) -> Vec<ModuleId> {
        self.as_ref()
            .provider_dependencies(module_id, trait_impls_only)
    }
}

#[derive(Debug, Clone, PartialEq)]
/// A declared child module recorded in the graph.
pub struct ModuleDeclaration {
    /// Declaration name.
    pub name: SymbolId,
    /// Source visibility of the declaration.
    pub visibility: Visibility,
    /// Interned child module id.
    pub target: ModuleId,
    /// Source span of the declaration.
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
/// A module declaration resolved before graph interning.
pub struct ResolvedModuleDeclaration {
    /// Declaration name.
    pub name: SymbolId,
    /// Source visibility of the declaration.
    pub visibility: Visibility,
    /// Source span of the declaration.
    pub span: Span,
}

/// Resolves module declarations from an active item tree.
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

/// Resolves module declarations with an explicit symbol provider for diagnostics.
pub fn resolve_module_declarations_from_active_item_tree_with_symbols(
    diagnostics: &mut Vec<Diagnostic>,
    item_tree: &ActiveModuleItemTree,
    symbols: &dyn SymbolText,
) -> Vec<ResolvedModuleDeclaration> {
    let mut seen = SymbolMap::<Span>::default();
    let mut declarations = Vec::new();
    for item in item_tree.items.iter() {
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
            visibility: item.vis,
            span: item.span,
        });
    }
    declarations
}

/// Adds resolved declarations to a graph, reporting invalid parent lookups.
pub fn add_resolved_module_declarations(
    graph: &mut ModuleGraph,
    module_id: ModuleId,
    declarations: impl IntoIterator<Item = ResolvedModuleDeclaration>,
) -> Result<(), ModuleGraphError> {
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

/// Checks whether an item visibility permits module access.
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
        Visibility::PublicRoot => {
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

/// Checks module-declaration visibility, including private descendants.
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

/// Computes a child source path using known symbol names.
pub fn declared_child_source_path(parent: &ModuleNode, child: SymbolId) -> SourcePath {
    declared_child_source_path_with_symbols(&KnownSymbolText, parent, child)
}

/// Computes a child source path from explicit parent identities.
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

/// Computes a child source path with an explicit symbol provider.
pub fn declared_child_source_path_with_symbols(
    symbols: &dyn SymbolText,
    parent: &ModuleNode,
    child: SymbolId,
) -> SourcePath {
    declared_child_source_path_for_with_symbols(symbols, &parent.path, &parent.module_path, child)
}

/// Computes a child source path from explicit identities and symbols.
///
/// A module-tree root lays out its children beside itself, whatever its file
/// name; any other module nests them under a directory of its own name.
pub fn declared_child_source_path_for_with_symbols(
    symbols: &dyn SymbolText,
    parent_path: &SourcePath,
    parent_module_path: &ModulePath,
    child: SymbolId,
) -> SourcePath {
    let sibling = parent_module_path.is_package_root();
    declared_child_source_path_with_layout(symbols, parent_path, child, sibling)
}

fn declared_child_source_path_with_layout(
    symbols: &dyn SymbolText,
    parent_path: &SourcePath,
    child: SymbolId,
    sibling: bool,
) -> SourcePath {
    let child = resolved_module_symbol_text(symbols, child);
    let physical_parent = parent_path.as_str();
    let logical_parent = parent_path.identity_ref().normalized_path();
    let physical = declared_child_path_text(physical_parent, &child, sibling);
    if physical_parent == logical_parent {
        return SourcePath::from_normalized_unchecked(physical);
    }
    let logical = declared_child_path_text(logical_parent, &child, sibling);
    SourcePath::with_normalized_identity_unchecked(physical, logical)
}

fn declared_child_path_text(parent_path: &str, child: &str, sibling: bool) -> String {
    let base = if sibling {
        parent_path.rsplit_once('/').map_or("", |(dir, _)| dir)
    } else {
        parent_path.strip_suffix(".nia").unwrap_or(parent_path)
    };
    let mut path = String::with_capacity(
        base.len() + usize::from(!base.is_empty()) + child.len() + ".nia".len(),
    );
    if !base.is_empty() {
        path.push_str(base);
        path.push('/');
    }
    path.push_str(child);
    path.push_str(".nia");
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_graph_indexes_paths_by_source_identity() {
        let mut graph =
            ModuleGraph::new(SourcePath::new("src/./main.nia")).expect("create module graph");
        let entry = graph.entry();
        let entry_source = graph.get(entry).expect("entry module").source_id;

        assert_eq!(graph.module_id_for_path("src/main.nia"), Some(entry));
        assert_eq!(graph.module_id_for_source_id(entry_source), Some(entry));
        assert_eq!(entry_source.store_id(), graph.source_store_id());
        let foreign_source = SourceTable::new()
            .id_for_path(&SourcePath::new("src/main.nia"))
            .expect("foreign source id");
        assert_eq!(foreign_source.local_index(), entry_source.local_index());
        assert_eq!(graph.module_id_for_source_id(foreign_source), None);
        let entry_key = graph.stable_key(entry).expect("entry stable key");
        assert_eq!(graph.module_id_for_stable_key(entry_key), Some(entry));
        let package = graph
            .intern_package_root(
                &module_root_symbol_from_text("root"),
                SourcePath::new("pkg/./root.nia"),
            )
            .expect("intern package root");
        assert_eq!(
            graph
                .intern_package_root(
                    &module_root_symbol_from_text("pkg_alias"),
                    SourcePath::new("pkg/root.nia")
                )
                .expect("intern package alias"),
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
    fn the_entry_file_roots_the_program_tree_and_lays_children_beside_it() {
        let mut graph = ModuleGraph::new(SourcePath::new("src/main.nia")).expect("module graph");
        let entry = graph.entry();
        assert_eq!(graph.package_root(&known::PROGRAM_TREE), Some(entry));
        assert_eq!(
            graph.root_module_for_segment(entry, ModuleRootSegment::Root),
            Some(entry)
        );
        // `entry` is no longer a path root; it is an ordinary name.
        assert_eq!(
            graph.root_module_for_name(entry, module_root_symbol_from_text("entry")),
            None
        );

        // A root's children sit beside it; a nested module's children nest
        // under a directory of its own name.
        let child = graph
            .intern_declared_child(entry, &known::START, Visibility::Public, Span::default())
            .expect("child module");
        assert_eq!(
            graph.get(child).expect("child").path.as_str(),
            "src/start.nia"
        );
        assert_eq!(
            graph
                .declared_child_source_path(graph.get(child).expect("child"), known::MAIN)
                .as_str(),
            "src/start/main.nia"
        );
    }

    #[test]
    fn any_tree_root_lays_children_beside_it_whatever_its_file_name() {
        let mut graph = ModuleGraph::new(SourcePath::new("src/main.nia")).expect("module graph");
        let library = graph
            .intern_package_root(
                &module_root_symbol_from_text("math"),
                SourcePath::new("vendor/math/src/root.nia"),
            )
            .expect("library root");
        assert_eq!(
            graph
                .declared_child_source_path(graph.get(library).expect("library"), known::START)
                .as_str(),
            "vendor/math/src/start.nia"
        );
    }

    #[test]
    fn declared_child_paths_preserve_relocated_source_coordinates() {
        let parent =
            SourcePath::with_identity("/opt/nia/lib/std/root.nia", "toolchain:/std/root.nia");
        let child = declared_child_source_path_for(
            &parent,
            &ModulePath::root(STD_MODULE_MAP_NAME),
            known::START,
        );

        assert_eq!(child.as_str(), "/opt/nia/lib/std/start.nia");
        assert_eq!(
            child.identity_ref().normalized_path(),
            "toolchain:/std/start.nia"
        );
    }

    #[test]
    fn module_graph_rejects_foreign_handles_with_matching_local_indices() {
        let graph = ModuleGraph::new(SourcePath::new("main.nia")).expect("create module graph");
        let foreign =
            ModuleGraph::new(SourcePath::new("foreign.nia")).expect("create module graph");

        assert_eq!(graph.entry().local_index(), foreign.entry().local_index());
        assert_ne!(graph.entry(), foreign.entry());
        assert!(graph.get(foreign.entry()).is_none());
    }

    #[test]
    fn stable_definition_keys_remap_across_graph_owners() {
        let first =
            ModuleGraph::new(SourcePath::new("src/./main.nia")).expect("create module graph");
        let second =
            ModuleGraph::new(SourcePath::new("src/main.nia")).expect("create module graph");
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
        let graph = ModuleGraph::new(SourcePath::new("main.nia")).expect("create module graph");
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
        let mut graph = ModuleGraph::new(SourcePath::new("main.nia")).expect("create module graph");
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

    #[test]
    fn module_map_rejects_reserved_roots_and_preserves_default_entries() {
        let mut map = ModuleMap::new();
        let package_path = SourcePath::new("deps/pkg/root.nia");
        map.try_insert("vendor", package_path.clone())
            .expect("ordinary package root");
        assert_eq!(map.get("vendor"), Some(&package_path));
        for reserved in COMPILER_RESERVED_MODULE_ROOTS {
            let error = map
                .try_insert(*reserved, SourcePath::new("reserved.nia"))
                .expect_err("reserved package root");
            assert!(error.contains(reserved));
        }

        // `entry` is an ordinary name, so a package may use it.
        map.try_insert("entry", SourcePath::new("deps/entry.nia"))
            .expect("entry is not reserved");
        let std = map.with_default_std(SourcePath::new("stdlib/root.nia"));
        assert_eq!(std.std_path(), Some(&SourcePath::new("stdlib/root.nia")));
        let preserved = std.with_default_std(SourcePath::new("other/std.nia"));
        assert_eq!(
            preserved.std_path(),
            Some(&SourcePath::new("stdlib/root.nia"))
        );
        assert!(!preserved.is_empty());
    }

    #[test]
    fn module_map_scoped_aliases_are_isolated_by_package_source() {
        let first = SourceIdentity::new("build-package:workspace/first:/src/main.nia");
        let second = SourceIdentity::new("build-package:workspace/second:/src/main.nia");
        let mut map = ModuleMap::new();
        map.add_scope(&SourceIdentity::new("build-package:workspace/first:/"));
        map.add_scope(&SourceIdentity::new("build-package:workspace/second:/"));
        map.try_insert("buildScript", SourcePath::new("build.nia"))
            .expect("explicit global root");
        map.insert_scoped(
            &SourceIdentity::new("build-package:workspace/first:/"),
            "shared",
            SourcePath::with_identity("/deps/first/root.nia", "build-package:first:/src/root.nia"),
        )
        .expect("first package alias");
        map.insert_scoped(
            &SourceIdentity::new("build-package:workspace/second:/"),
            "shared",
            SourcePath::with_identity(
                "/deps/second/root.nia",
                "build-package:second:/src/root.nia",
            ),
        )
        .expect("second package alias");

        assert!(map.get("shared").is_none());
        assert!(
            map.for_source(&SourcePath::new(first.normalized_path()))
                .get("shared")
                .unwrap()
                .as_str()
                .contains("first")
        );
        assert!(
            map.for_source(&SourcePath::new(first.normalized_path()))
                .get("buildScript")
                .is_some()
        );
        assert!(
            map.for_source(&SourcePath::new(second.normalized_path()))
                .get("shared")
                .unwrap()
                .as_str()
                .contains("second")
        );
    }

    #[test]
    fn module_path_navigation_and_root_classification_are_stable() {
        let root = ModulePath {
            package: known::PROGRAM_TREE,
            segments: Vec::new(),
        };
        assert!(!ModulePath::root("entry").is_program_tree());
        assert!(root.is_package_root());
        assert!(root.is_program_tree());
        assert_eq!(root.parent(), None);

        let child = root.child(known::START);
        assert!(!child.is_package_root());
        assert_eq!(child.parent(), Some(root.clone()));
        // `runtime` is an ordinary name; the injected runtime tree has none.
        assert_ne!(ModulePath::root("runtime").package, known::RUNTIME_TREE);
    }

    #[test]
    fn unknown_module_symbols_keep_distinct_identity_text() {
        let first = SymbolId::from_stable_hash(0x1111);
        let second = SymbolId::from_stable_hash(0x2222);
        assert_eq!(module_symbol_text(known::PROGRAM), "program");
        assert_eq!(module_symbol_text(first), "sym:0000000000001111");
        assert_ne!(module_symbol_text(first), module_symbol_text(second));

        let parent = ModulePath {
            package: known::PROGRAM_TREE,
            segments: Vec::new(),
        };
        let source = SourcePath::new("src/main.nia");
        let first_path = declared_child_source_path_for(&source, &parent, first);
        let second_path = declared_child_source_path_for(&source, &parent, second);
        assert_eq!(first_path.as_str(), "src/sym:0000000000001111.nia");
        assert_ne!(first_path.identity(), second_path.identity());

        let package_root = ModulePath::root("runtime");
        let package_path = SourcePath::new("lib/runtime/root.nia");
        let child_path = declared_child_source_path_for(&package_path, &package_root, known::START);
        assert_eq!(child_path.as_str(), "lib/runtime/start.nia");
    }

    #[test]
    fn package_aliases_and_root_selectors_resolve_to_reused_modules() {
        let mut graph =
            ModuleGraph::new(SourcePath::new("src/main.nia")).expect("create module graph");
        let entry = graph.entry();
        let package = SymbolId::from_stable_hash(stable_hash("dependency"));
        let root = graph
            .intern_package_root(&package, SourcePath::new("deps/root.nia"))
            .expect("intern package root");
        let alias = SymbolId::from_stable_hash(stable_hash("dependency_alias"));
        assert_eq!(
            graph
                .intern_package_root(&alias, SourcePath::new("deps/./root.nia"))
                .expect("intern package alias"),
            root
        );
        assert_eq!(graph.package_root(&package), Some(root));
        assert_eq!(graph.package_root(&alias), Some(root));
        assert_eq!(
            graph.root_module_for_segment(entry, ModuleRootSegment::Named(alias)),
            Some(root)
        );
        assert_eq!(
            graph.root_module_for_segment(entry, ModuleRootSegment::Root),
            Some(entry)
        );
        assert_eq!(
            graph.root_module_for_segment(entry, ModuleRootSegment::Parent),
            None
        );
    }

    #[test]
    fn visibility_and_module_declaration_boundaries_follow_module_ancestry() {
        let mut graph = ModuleGraph::new(SourcePath::new("main.nia")).expect("create module graph");
        let entry = graph.entry();
        let parent_name = SymbolId::from_stable_hash(stable_hash("parent"));
        let sibling_name = SymbolId::from_stable_hash(stable_hash("sibling"));
        let child_name = SymbolId::from_stable_hash(stable_hash("child"));
        let parent = graph
            .intern_declared_child(entry, &parent_name, Visibility::Public, Span::default())
            .expect("parent module");
        let sibling = graph
            .intern_declared_child(entry, &sibling_name, Visibility::Public, Span::default())
            .expect("sibling module");
        let child = graph
            .intern_declared_child(parent, &child_name, Visibility::Private, Span::default())
            .expect("nested module");

        assert!(visibility_allows(
            Visibility::Private,
            &graph,
            parent,
            parent
        ));
        assert!(!visibility_allows(
            Visibility::Private,
            &graph,
            parent,
            sibling
        ));
        assert!(visibility_allows(
            Visibility::PublicSuper,
            &graph,
            parent,
            entry
        ));
        assert!(visibility_allows(
            Visibility::PublicSuper,
            &graph,
            parent,
            sibling
        ));
        assert!(visibility_allows(
            Visibility::PublicSuper,
            &graph,
            parent,
            child
        ));
        assert!(module_declaration_visibility_allows(
            Visibility::Private,
            &graph,
            parent,
            child,
        ));
        assert!(!module_declaration_visibility_allows(
            Visibility::Private,
            &graph,
            parent,
            sibling,
        ));

        let package = SymbolId::from_stable_hash(stable_hash("other"));
        let other_root = graph
            .intern_package_root(&package, SourcePath::new("other/root.nia"))
            .expect("intern package root");
        assert!(visibility_allows(
            Visibility::Public,
            &graph,
            parent,
            other_root
        ));
        assert!(!visibility_allows(
            Visibility::PublicRoot,
            &graph,
            parent,
            other_root
        ));
        assert!(visibility_allows(
            Visibility::PublicRoot,
            &graph,
            parent,
            child
        ));
    }

    #[test]
    fn module_processing_flags_are_idempotent_and_duplicate_declarations_error() {
        let mut graph = ModuleGraph::new(SourcePath::new("main.nia")).expect("create module graph");
        let entry = graph.entry();
        let child_name = known::START;
        let child = graph
            .intern_declared_child_with_processing(
                entry,
                &child_name,
                Visibility::Public,
                Span::default(),
                false,
                false,
            )
            .expect("deferred child");
        let node = graph.get(child).expect("child node");
        assert!(!node.semantic_selected);
        assert!(!node.process_used_paths);
        assert!(!node.process_declared_children);
        assert!(graph.mark_process_used_paths(child));
        assert!(!graph.mark_process_used_paths(child));
        assert!(!graph.mark_semantic_selected(child));
        graph.mark_process_declared_children(child);
        assert!(
            graph
                .get(child)
                .expect("selected child")
                .process_declared_children
        );

        let duplicate = graph
            .intern_declared_child(entry, &child_name, Visibility::Public, Span::new(1, 2))
            .expect_err("duplicate declaration");
        let ModuleGraphError::Diagnostic(duplicate) = duplicate else {
            panic!("duplicate declaration must remain a user diagnostic");
        };
        assert_eq!(duplicate.summary, "duplicate module declaration `start`");
    }
}
