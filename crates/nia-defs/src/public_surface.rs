// SPDX-License-Identifier: GPL-3.0-or-later
use std::{collections::HashMap, sync::Arc};

use crate::DefId;
use nia_ids::ModuleId;
use nia_span::Span;
use nia_symbol::{SymbolId, SymbolMap, SymbolSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PublicNamespace {
    Value,
    Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicSource {
    Direct,
    PubUsing { directive_span: Span },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicItem {
    pub target_module: nia_ids::ModuleId,
    pub target_def_id: DefId,
    pub namespace: PublicNamespace,
    pub name_span: Span,
    pub source: PublicSource,
    /// When this item is an enum variant, the GlobalDefId of the parent enum.
    /// `None` for ordinary function / global / type / variant-from-a-non-enum cases.
    pub parent_enum: Option<nia_ids::GlobalDefId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModulePublicSurface {
    pub module_id: ModuleId,
    pub modules: SymbolMap<ModuleId>,
    pub values: SymbolMap<PublicItem>,
    pub types: SymbolMap<PublicItem>,
}

impl ModulePublicSurface {
    pub fn new(module_id: ModuleId) -> Self {
        Self {
            module_id,
            modules: SymbolMap::default(),
            values: SymbolMap::default(),
            types: SymbolMap::default(),
        }
    }

    pub fn lookup_module(&self, name: &SymbolId) -> Option<ModuleId> {
        self.modules.get(name).copied()
    }

    pub fn lookup_value(&self, name: &SymbolId) -> Option<&PublicItem> {
        self.values.get(name)
    }

    pub fn lookup_type(&self, name: &SymbolId) -> Option<&PublicItem> {
        self.types.get(name)
    }

    pub fn lookup(&self, namespace: PublicNamespace, name: &SymbolId) -> Option<&PublicItem> {
        match namespace {
            PublicNamespace::Value => self.lookup_value(name),
            PublicNamespace::Type => self.lookup_type(name),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PublicSurfaces {
    per_module: HashMap<ModuleId, Arc<ModulePublicSurface>>,
}

pub trait PublicSurfaceLookup {
    fn public_surface(&self, module_id: ModuleId) -> Option<Arc<ModulePublicSurface>>;

    fn public_module(&self, module_id: ModuleId, name: &SymbolId) -> Option<ModuleId> {
        self.public_surface(module_id)?.lookup_module(name)
    }

    fn public_value(&self, module_id: ModuleId, name: &SymbolId) -> Option<PublicItem> {
        self.public_surface(module_id)?.lookup_value(name).cloned()
    }

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
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, surface: ModulePublicSurface) {
        self.per_module.insert(surface.module_id, Arc::new(surface));
    }

    pub fn get(&self, module_id: ModuleId) -> Option<&ModulePublicSurface> {
        self.per_module.get(&module_id).map(Arc::as_ref)
    }

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsingEntry {
    pub target_module: ModuleId,
    pub target_def_id: DefId,
    pub namespace: PublicNamespace,
    pub directive_span: Span,
    pub name_span: Span,
    /// When the imported item is an enum variant, the GlobalDefId of its parent enum.
    pub parent_enum: Option<nia_ids::GlobalDefId>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModuleUsingScope {
    pub modules: SymbolMap<ModuleId>,
    pub values: SymbolMap<UsingEntry>,
    pub types: SymbolMap<UsingEntry>,
    pub unresolved_names: SymbolSet,
}

pub trait UsingScopeLookup {
    fn using_module(&self, name: &SymbolId) -> Option<ModuleId>;
    fn using_value(&self, name: &SymbolId) -> Option<UsingEntry>;
    fn using_type(&self, name: &SymbolId) -> Option<UsingEntry>;
    fn has_unresolved_using_name(&self, name: &SymbolId) -> bool;
}

impl ModuleUsingScope {
    pub fn lookup_module(&self, name: &SymbolId) -> Option<ModuleId> {
        self.modules.get(name).copied()
    }

    pub fn lookup_value(&self, name: &SymbolId) -> Option<&UsingEntry> {
        self.values.get(name)
    }

    pub fn lookup_type(&self, name: &SymbolId) -> Option<&UsingEntry> {
        self.types.get(name)
    }

    pub fn has_unresolved_name(&self, name: &SymbolId) -> bool {
        self.unresolved_names.contains(name)
    }

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
