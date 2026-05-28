// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::DefId;
use nia_ids::ModuleId;
use nia_span::Span;

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
    pub source: PublicSource,
    /// When this item is an enum variant, the GlobalDefId of the parent enum.
    /// `None` for ordinary function / global / type / variant-from-a-non-enum cases.
    pub parent_enum: Option<nia_ids::GlobalDefId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModulePublicSurface {
    pub module_id: ModuleId,
    pub modules: HashMap<String, ModuleId>,
    pub values: HashMap<String, PublicItem>,
    pub types: HashMap<String, PublicItem>,
}

impl ModulePublicSurface {
    pub fn new(module_id: ModuleId) -> Self {
        Self {
            module_id,
            modules: HashMap::new(),
            values: HashMap::new(),
            types: HashMap::new(),
        }
    }

    pub fn lookup_module(&self, name: &str) -> Option<ModuleId> {
        self.modules.get(name).copied()
    }

    pub fn lookup_value(&self, name: &str) -> Option<&PublicItem> {
        self.values.get(name)
    }

    pub fn lookup_type(&self, name: &str) -> Option<&PublicItem> {
        self.types.get(name)
    }

    pub fn lookup(&self, namespace: PublicNamespace, name: &str) -> Option<&PublicItem> {
        match namespace {
            PublicNamespace::Value => self.lookup_value(name),
            PublicNamespace::Type => self.lookup_type(name),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PublicSurfaces {
    per_module: HashMap<ModuleId, ModulePublicSurface>,
}

impl PublicSurfaces {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, surface: ModulePublicSurface) {
        self.per_module.insert(surface.module_id, surface);
    }

    pub fn get(&self, module_id: ModuleId) -> Option<&ModulePublicSurface> {
        self.per_module.get(&module_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ModuleId, &ModulePublicSurface)> {
        self.per_module.iter()
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
    pub modules: HashMap<String, ModuleId>,
    pub values: HashMap<String, UsingEntry>,
    pub types: HashMap<String, UsingEntry>,
}

impl ModuleUsingScope {
    pub fn lookup_module(&self, name: &str) -> Option<ModuleId> {
        self.modules.get(name).copied()
    }

    pub fn lookup_value(&self, name: &str) -> Option<&UsingEntry> {
        self.values.get(name)
    }

    pub fn lookup_type(&self, name: &str) -> Option<&UsingEntry> {
        self.types.get(name)
    }

    pub fn entries(&self) -> impl Iterator<Item = (&str, &UsingEntry)> {
        self.values
            .iter()
            .chain(self.types.iter())
            .map(|(name, entry)| (name.as_str(), entry))
    }
}
