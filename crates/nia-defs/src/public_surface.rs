// SPDX-License-Identifier: GPL-3.0-or-later
use std::{collections::HashMap, sync::Arc};

use crate::DefId;
use nia_ids::ModuleId;
use nia_span::Span;
use nia_symbol::{SymbolId, SymbolMap, SymbolSet};

/// Public item namespace used by import and re-export lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PublicNamespace {
    /// Value namespace.
    Value,
    /// Type namespace.
    Type,
}

/// How an item entered a module's public surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicSource {
    /// Declared directly in the module.
    Direct,
    /// Re-exported by a public `using` directive.
    PubUsing {
        /// Source span of the re-exporting directive.
        directive_span: Span,
    },
}

/// One value or type exported from a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicItem {
    /// Module defining the target item.
    pub target_module: nia_ids::ModuleId,
    /// Target definition id within `target_module`.
    pub target_def_id: DefId,
    /// Namespace in which the item is exported.
    pub namespace: PublicNamespace,
    /// Span of the exported name at its source.
    pub name_span: Span,
    /// Direct or re-export provenance.
    pub source: PublicSource,
    /// When this item is an enum variant, the GlobalDefId of the parent enum.
    /// `None` for ordinary function / global / type / variant-from-a-non-enum cases.
    pub parent_enum: Option<nia_ids::GlobalDefId>,
}

/// Complete public module, value, and type namespaces for one module.
#[derive(Debug, Clone, PartialEq)]
pub struct ModulePublicSurface {
    /// Module owning this surface.
    pub module_id: ModuleId,
    /// Exported child modules by name.
    pub modules: SymbolMap<ModuleId>,
    /// Exported values by name.
    pub values: SymbolMap<PublicItem>,
    /// Exported types by name.
    pub types: SymbolMap<PublicItem>,
}

impl ModulePublicSurface {
    /// Creates an empty public surface for `module_id`.
    pub fn new(module_id: ModuleId) -> Self {
        Self {
            module_id,
            modules: SymbolMap::default(),
            values: SymbolMap::default(),
            types: SymbolMap::default(),
        }
    }

    /// Looks up an exported module.
    pub fn lookup_module(&self, name: &SymbolId) -> Option<ModuleId> {
        self.modules.get(name).copied()
    }

    /// Looks up an exported value.
    pub fn lookup_value(&self, name: &SymbolId) -> Option<&PublicItem> {
        self.values.get(name)
    }

    /// Looks up an exported type.
    pub fn lookup_type(&self, name: &SymbolId) -> Option<&PublicItem> {
        self.types.get(name)
    }

    /// Looks up an item in an explicit namespace.
    pub fn lookup(&self, namespace: PublicNamespace, name: &SymbolId) -> Option<&PublicItem> {
        match namespace {
            PublicNamespace::Value => self.lookup_value(name),
            PublicNamespace::Type => self.lookup_type(name),
        }
    }
}

/// In-memory collection of public surfaces keyed by module.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PublicSurfaces {
    per_module: HashMap<ModuleId, Arc<ModulePublicSurface>>,
}

/// Read-only resolver for module public surfaces.
pub trait PublicSurfaceLookup {
    /// Returns a module's public surface when available.
    fn public_surface(&self, module_id: ModuleId) -> Option<Arc<ModulePublicSurface>>;

    /// Looks up an exported child module.
    fn public_module(&self, module_id: ModuleId, name: &SymbolId) -> Option<ModuleId> {
        self.public_surface(module_id)?.lookup_module(name)
    }

    /// Looks up an exported value.
    fn public_value(&self, module_id: ModuleId, name: &SymbolId) -> Option<PublicItem> {
        self.public_surface(module_id)?.lookup_value(name).cloned()
    }

    /// Looks up an exported type.
    fn public_type(&self, module_id: ModuleId, name: &SymbolId) -> Option<PublicItem> {
        self.public_surface(module_id)?.lookup_type(name).cloned()
    }
}

impl<F> PublicSurfaceLookup for F
where
    F: Fn(ModuleId) -> Option<Arc<ModulePublicSurface>>,
{
    fn public_surface(&self, module_id: ModuleId) -> Option<Arc<ModulePublicSurface>> {
        self(module_id)
    }
}

impl PublicSurfaces {
    /// Creates an empty public-surface collection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts or replaces a module's public surface.
    pub fn insert(&mut self, surface: ModulePublicSurface) {
        self.per_module.insert(surface.module_id, Arc::new(surface));
    }

    /// Borrows a module's public surface.
    pub fn get(&self, module_id: ModuleId) -> Option<&ModulePublicSurface> {
        self.per_module.get(&module_id).map(Arc::as_ref)
    }

    /// Iterates all stored module surfaces.
    pub fn iter(&self) -> impl Iterator<Item = (&ModuleId, &ModulePublicSurface)> {
        self.per_module
            .iter()
            .map(|(module_id, surface)| (module_id, surface.as_ref()))
    }
}

impl PublicSurfaceLookup for PublicSurfaces {
    fn public_surface(&self, module_id: ModuleId) -> Option<Arc<ModulePublicSurface>> {
        self.per_module.get(&module_id).cloned()
    }
}

/// Resolved name imported into a module using scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsingEntry {
    /// Module defining the imported item.
    pub target_module: ModuleId,
    /// Target definition id within `target_module`.
    pub target_def_id: DefId,
    /// Imported namespace.
    pub namespace: PublicNamespace,
    /// Source span of the importing directive.
    pub directive_span: Span,
    /// Source span of the imported name.
    pub name_span: Span,
    /// When the imported item is an enum variant, the GlobalDefId of its parent enum.
    pub parent_enum: Option<nia_ids::GlobalDefId>,
}

/// Names made available by a module's resolved `using` directives.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModuleUsingScope {
    /// Imported modules by local name.
    pub modules: SymbolMap<ModuleId>,
    /// Imported values by local name.
    pub values: SymbolMap<UsingEntry>,
    /// Imported types by local name.
    pub types: SymbolMap<UsingEntry>,
    /// Names whose import resolution failed.
    pub unresolved_names: SymbolSet,
}

/// Read-only resolver for one module's using scope.
pub trait UsingScopeLookup {
    /// Looks up an imported module.
    fn using_module(&self, name: &SymbolId) -> Option<ModuleId>;
    /// Looks up an imported value.
    fn using_value(&self, name: &SymbolId) -> Option<UsingEntry>;
    /// Looks up an imported type.
    fn using_type(&self, name: &SymbolId) -> Option<UsingEntry>;
    /// Tests whether an unresolved import used `name`.
    fn has_unresolved_using_name(&self, name: &SymbolId) -> bool;
}

impl ModuleUsingScope {
    /// Looks up an imported module.
    pub fn lookup_module(&self, name: &SymbolId) -> Option<ModuleId> {
        self.modules.get(name).copied()
    }

    /// Looks up an imported value.
    pub fn lookup_value(&self, name: &SymbolId) -> Option<&UsingEntry> {
        self.values.get(name)
    }

    /// Looks up an imported type.
    pub fn lookup_type(&self, name: &SymbolId) -> Option<&UsingEntry> {
        self.types.get(name)
    }

    /// Tests whether resolution failed for `name`.
    pub fn has_unresolved_name(&self, name: &SymbolId) -> bool {
        self.unresolved_names.contains(name)
    }

    /// Iterates imported value and type entries.
    pub fn entries(&self) -> impl Iterator<Item = (&SymbolId, &UsingEntry)> {
        self.values.iter().chain(self.types.iter())
    }
}

impl UsingScopeLookup for ModuleUsingScope {
    fn using_module(&self, name: &SymbolId) -> Option<ModuleId> {
        self.lookup_module(name)
    }

    fn using_value(&self, name: &SymbolId) -> Option<UsingEntry> {
        self.lookup_value(name).cloned()
    }

    fn using_type(&self, name: &SymbolId) -> Option<UsingEntry> {
        self.lookup_type(name).cloned()
    }

    fn has_unresolved_using_name(&self, name: &SymbolId) -> bool {
        self.has_unresolved_name(name)
    }
}
