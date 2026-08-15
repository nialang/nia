// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{
    ArrayLen, BindingItem, BindingStmt, Block, Expr, ExprKind, FunctionItem, IndexArg,
    MatchArmBody, Module, Pattern, PatternKind, Stmt, StmtKind, TypeArg, TypeKind, TypeRef,
};
use nia_defs::DefCollection;
use nia_diagnostic::{Diagnostic, codes};
pub use nia_ids::LocalId;
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNode, ItemTreeNodeKind, ModuleItemTree};
use nia_node_id::{NodeMap, NodeMapBuilder, NodeStore, VersionedNodeKey};
use nia_span::Span;
use nia_symbol::{SymbolId, SymbolMap, SymbolText, symbol_text_from_optional_resolver};
use nia_value_resolve::{ValueNameResolution, ValueResolution};

mod allocation;
mod resolver;

use allocation::LocalDefinitionAllocator;
use resolver::{
    resolve_module_locals_from_filtered_items, resolve_module_locals_from_items,
    resolve_module_locals_from_items_with_symbols,
};

#[derive(Debug, Clone, PartialEq)]
pub struct LocalResolution {
    pub locals: LocalMap,
    pub node_local_defs: NodeMap<LocalId>,
    pub node_uses: NodeMap<LocalUse>,
    /// Exact nominal definitions for nodes classified as type prefixes.
    ///
    /// `LocalUse::TypePrefix` is intentionally only a use category. Consumers that lower a
    /// constructor-shaped expression, such as a struct pattern, also need its stable cross-module
    /// identity and must not reconstruct that identity from the source spelling.
    pub node_type_prefixes: NodeMap<nia_ids::GlobalDefId>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
pub struct LocalResolutionBuilder {
    locals: LocalMap,
    node_local_defs: NodeMapBuilder<LocalId>,
    node_uses: NodeMapBuilder<LocalUse>,
    node_type_prefixes: NodeMapBuilder<nia_ids::GlobalDefId>,
    diagnostics: Vec<Diagnostic>,
}

impl LocalResolution {
    pub fn with_store(store: &NodeStore) -> Self {
        Self {
            locals: LocalMap::default(),
            node_local_defs: NodeMap::with_store(store),
            node_uses: NodeMap::with_store(store),
            node_type_prefixes: NodeMap::with_store(store),
            diagnostics: Vec::new(),
        }
    }

    pub fn into_builder(self) -> LocalResolutionBuilder {
        LocalResolutionBuilder {
            locals: self.locals,
            node_local_defs: self.node_local_defs.into_builder(),
            node_uses: self.node_uses.into_builder(),
            node_type_prefixes: self.node_type_prefixes.into_builder(),
            diagnostics: self.diagnostics,
        }
    }
}

impl LocalResolutionBuilder {
    pub fn remove_node_local_def(&mut self, locator: &VersionedNodeKey) -> Option<LocalId> {
        self.node_local_defs.remove(locator)
    }

    pub fn finish(self) -> LocalResolution {
        LocalResolution {
            locals: self.locals,
            node_local_defs: self.node_local_defs.finish(),
            node_uses: self.node_uses.finish(),
            node_type_prefixes: self.node_type_prefixes.finish(),
            diagnostics: self.diagnostics,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LocalMap {
    locals: Vec<Local>,
}

impl LocalMap {
    pub fn get(&self, id: LocalId) -> Option<&Local> {
        self.locals.get(id.0 as usize)
    }

    pub fn iter(&self) -> impl Iterator<Item = (LocalId, &Local)> {
        self.locals
            .iter()
            .enumerate()
            .map(|(index, local)| (LocalId(index as u32), local))
    }

    pub fn len(&self) -> usize {
        self.locals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.locals.is_empty()
    }

    fn push(&mut self, local: Local) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(local);
        id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Local {
    pub name: LocalBindingName,
    pub kind: LocalKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalBindingName {
    Named(SymbolId),
    SelfValue,
}

impl LocalBindingName {
    pub fn named(name: SymbolId) -> Self {
        Self::Named(name)
    }

    pub fn symbol(self) -> Option<SymbolId> {
        match self {
            Self::Named(name) => Some(name),
            Self::SelfValue => None,
        }
    }

    pub fn is_self_value(self) -> bool {
        matches!(self, Self::SelfValue)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalKind {
    Param,
    MutableBinding,
    ImmutableBinding,
    ConstBinding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalUse {
    Local(LocalId),
    Static(nia_ids::GlobalDefId),
    ModuleValue,
    Module,
    TypePrefix,
    Unresolved,
}

pub fn resolve_module_locals(
    module: &Module,
    defs: &DefCollection,
    values: &ValueResolution,
) -> LocalResolution {
    let item_tree = ModuleItemTree::from_module(module);
    resolve_module_locals_from_item_tree(&item_tree, defs, values)
}

pub fn resolve_module_locals_with_source(
    module: &Module,
    defs: &DefCollection,
    values: &ValueResolution,
    _source_version: Option<nia_source::SourceVersion>,
) -> LocalResolution {
    let item_tree = ModuleItemTree::from_module(module);
    let node_store = NodeStore::new();
    resolve_module_locals_from_items(&item_tree.items, defs, values, &node_store)
}

pub fn resolve_module_locals_with_origins(
    module: &Module,
    defs: &DefCollection,
    values: &ValueResolution,
    _source_version: Option<nia_source::SourceVersion>,
    origins: &nia_node_id::NodeOriginTable,
) -> LocalResolution {
    let item_tree = ModuleItemTree::from_module(module);
    resolve_module_locals_from_items(&item_tree.items, defs, values, origins.node_store())
}

pub fn resolve_module_locals_from_item_tree(
    item_tree: &ModuleItemTree,
    defs: &DefCollection,
    values: &ValueResolution,
) -> LocalResolution {
    let node_store = NodeStore::new();
    resolve_module_locals_from_items(&item_tree.items, defs, values, &node_store)
}

pub fn resolve_module_locals_from_active_item_tree_with_origins(
    item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    values: &ValueResolution,
    _source_version: Option<nia_source::SourceVersion>,
    origins: &nia_node_id::NodeOriginTable,
) -> LocalResolution {
    resolve_module_locals_from_items(&item_tree.items, defs, values, origins.node_store())
}

pub fn resolve_module_locals_from_active_item_tree_with_origins_and_symbols(
    item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    values: &ValueResolution,
    _source_version: Option<nia_source::SourceVersion>,
    origins: &nia_node_id::NodeOriginTable,
    symbols: &dyn SymbolText,
) -> LocalResolution {
    resolve_module_locals_from_items_with_symbols(
        &item_tree.items,
        defs,
        values,
        Some(symbols),
        origins.node_store(),
    )
}

pub fn resolve_module_locals_from_filtered_active_item_tree_with_origins(
    filtered_item_tree: &ActiveModuleItemTree,
    full_item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    values: &ValueResolution,
    _source_version: Option<nia_source::SourceVersion>,
    origins: &nia_node_id::NodeOriginTable,
) -> LocalResolution {
    resolve_module_locals_from_filtered_items(
        &filtered_item_tree.items,
        &full_item_tree.items,
        defs,
        values,
        None,
        origins.node_store(),
    )
}

pub fn resolve_module_locals_from_filtered_active_item_tree_with_origins_and_symbols(
    filtered_item_tree: &ActiveModuleItemTree,
    full_item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    values: &ValueResolution,
    _source_version: Option<nia_source::SourceVersion>,
    origins: &nia_node_id::NodeOriginTable,
    symbols: &dyn SymbolText,
) -> LocalResolution {
    resolve_module_locals_from_filtered_items(
        &filtered_item_tree.items,
        &full_item_tree.items,
        defs,
        values,
        Some(symbols),
        origins.node_store(),
    )
}

pub fn resolve_module_locals_from_item_tree_with_origins(
    item_tree: &ModuleItemTree,
    defs: &DefCollection,
    values: &ValueResolution,
    _source_version: Option<nia_source::SourceVersion>,
    _origins: (),
) -> LocalResolution {
    let node_store = NodeStore::new();
    resolve_module_locals_from_items(&item_tree.items, defs, values, &node_store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{collect_module_defs, collect_module_defs_from_active_item_tree};
    use nia_ids::{DefId, GlobalDefId, ModuleIdAllocator};
    use nia_item_tree::ModuleItemTree;
    use nia_node_id::{NodePosition, SyntaxKind};
    use nia_parser::{parse_module, parse_module_syntax_with_origins};
    use nia_source::{SourceId, SourceRevision, SourceVersion};
    use nia_symbol::stable_hash;
    use nia_value_resolve::{
        ProgramDefsContext as ValueProgramDefsContext, resolve_module_values,
        resolve_module_values_from_active_item_tree,
    };

    include!("tests/local_resolve/test_support.rs");

    #[path = "local_resolve/lexical_bindings.rs"]
    mod lexical_bindings;

    #[path = "local_resolve/source_key_facts.rs"]
    mod source_key_facts;

    #[path = "local_resolve/diagnostic_contracts.rs"]
    mod diagnostic_contracts;

    #[path = "local_resolve/bracket_suffixes.rs"]
    mod bracket_suffixes;

    #[path = "local_resolve/active_item_tree.rs"]
    mod active_item_tree;
}
