// SPDX-License-Identifier: GPL-3.0-or-later
use std::{collections::HashMap, sync::Arc};

use nia_ast::{
    ArrayLen, AssocBindingKey, FunctionItem, GenericParam, GenericParamKind, Item, ItemKind,
    Module, PathSegmentKind, TypeArg, TypeKind, TypePathSegment, TypeRef,
};
use nia_ast_walk::{Visitor, walk_function, walk_item};
use nia_defs::{DefCollection, DefKind, PublicNamespace, PublicSurfaceLookup, UsingScopeLookup};
use nia_diagnostic::{Diagnostic, codes};
pub use nia_ids::DefId;
use nia_ids::{GlobalDefId, ModuleId, Visibility};
use nia_imports::{
    ModuleGraph, ModuleGraphLookup, ModuleRootSegment, module_declaration_visibility_allows,
    visibility_allows,
};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNode, ItemTreeNodeKind, ModuleItemTree};
use nia_node_id::{NodeMap, NodeSite, NodeStore, VersionedNodeKey};
use nia_span::Span;
use nia_symbol::{SymbolId, SymbolText, known, symbol_text_from_optional_resolver};
use nia_ty::{BuiltinTrait, PrimitiveTy, PrimitiveTypeSpelling};

mod resolver;

use resolver::{
    TypeResolveMode, TypeResolveOptions, resolve_module_types_from_item_tree_inner,
    resolve_module_types_from_items_with_mode,
};

#[derive(Debug, Clone, PartialEq)]
pub struct TypeResolution {
    pub node_type_names: HashMap<NodeSite, TypeNameResolution>,
    pub node_qualified_type_names: HashMap<NodeSite, GlobalDefId>,
    pub node_const_generic_names: NodeMap<SymbolId>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeNameResolution {
    Primitive(PrimitiveTypeSpelling),
    BuiltinTrait(BuiltinTrait),
    Def(DefId),
    External(GlobalDefId),
    GenericParam,
    AssociatedType,
    Error,
}

#[derive(Clone, Copy)]
pub struct ProgramDefsContext<'a> {
    pub defs: Option<&'a dyn Fn(ModuleId) -> Option<Arc<DefCollection>>>,
    pub graph: Option<&'a dyn ModuleGraphLookup>,
}

impl<'a> ProgramDefsContext<'a> {
    pub fn empty() -> Self {
        Self {
            defs: None,
            graph: None,
        }
    }
}

impl std::fmt::Debug for ProgramDefsContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgramDefsContext")
            .field("defs", &self.defs.is_some())
            .field("graph", &self.graph.is_some())
            .finish()
    }
}

enum ModuleDefs<'a> {
    Borrowed(&'a DefCollection),
    Shared(Arc<DefCollection>),
}

impl ModuleDefs<'_> {
    fn as_ref(&self) -> &DefCollection {
        match self {
            ModuleDefs::Borrowed(defs) => defs,
            ModuleDefs::Shared(defs) => defs,
        }
    }
}

pub fn resolve_module_types(module: &Module, defs: &DefCollection) -> TypeResolution {
    let item_tree = ModuleItemTree::from_module(module);
    resolve_module_types_from_item_tree(&item_tree, defs)
}

pub fn resolve_module_types_with_symbols(
    module: &Module,
    defs: &DefCollection,
    symbols: &dyn SymbolText,
) -> TypeResolution {
    let item_tree = ModuleItemTree::from_module(module);
    resolve_module_types_from_item_tree_inner(
        &item_tree,
        defs,
        None,
        ProgramDefsContext::empty(),
        None,
        None,
        Some(symbols),
    )
}

pub fn resolve_module_types_with_graph(
    module: &Module,
    defs: &DefCollection,
    graph: &ModuleGraph,
    program_defs: ProgramDefsContext<'_>,
) -> TypeResolution {
    let item_tree = ModuleItemTree::from_module(module);
    resolve_module_types_from_item_tree_inner(
        &item_tree,
        defs,
        Some(graph),
        program_defs,
        None,
        None,
        None,
    )
}

pub fn resolve_module_types_with_context(
    module: &Module,
    defs: &DefCollection,
    graph: &ModuleGraph,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &dyn PublicSurfaceLookup,
    using_scope: &dyn UsingScopeLookup,
) -> TypeResolution {
    let item_tree = ModuleItemTree::from_module(module);
    resolve_module_types_from_item_tree_inner(
        &item_tree,
        defs,
        Some(graph),
        program_defs,
        Some(public_surfaces),
        Some(using_scope),
        None,
    )
}

pub fn resolve_module_types_from_item_tree(
    item_tree: &ModuleItemTree,
    defs: &DefCollection,
) -> TypeResolution {
    resolve_module_types_from_item_tree_inner(
        item_tree,
        defs,
        None,
        ProgramDefsContext::empty(),
        None,
        None,
        None,
    )
}

pub fn resolve_module_types_from_active_item_tree(
    item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &dyn PublicSurfaceLookup,
    using_scope: &dyn UsingScopeLookup,
) -> TypeResolution {
    let node_store = NodeStore::new();
    resolve_module_types_from_items_with_mode(
        &item_tree.items,
        defs,
        program_defs.graph,
        program_defs,
        Some(public_surfaces),
        Some(using_scope),
        TypeResolveOptions {
            symbols: None,
            mode: TypeResolveMode::All,
            node_store: &node_store,
        },
    )
}

pub fn resolve_module_declaration_types_from_active_item_tree(
    item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &dyn PublicSurfaceLookup,
    using_scope: &dyn UsingScopeLookup,
) -> TypeResolution {
    let node_store = NodeStore::new();
    resolve_module_types_from_items_with_mode(
        &item_tree.items,
        defs,
        program_defs.graph,
        program_defs,
        Some(public_surfaces),
        Some(using_scope),
        TypeResolveOptions {
            symbols: None,
            mode: TypeResolveMode::Declarations,
            node_store: &node_store,
        },
    )
}

pub fn resolve_module_types_from_active_item_tree_with_symbols(
    item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &dyn PublicSurfaceLookup,
    using_scope: &dyn UsingScopeLookup,
    symbols: &dyn SymbolText,
) -> TypeResolution {
    let node_store = NodeStore::new();
    resolve_module_types_from_active_item_tree_with_symbols_in_store(
        item_tree,
        defs,
        program_defs,
        public_surfaces,
        using_scope,
        symbols,
        &node_store,
    )
}

pub fn resolve_module_types_from_active_item_tree_with_symbols_in_store(
    item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &dyn PublicSurfaceLookup,
    using_scope: &dyn UsingScopeLookup,
    symbols: &dyn SymbolText,
    node_store: &NodeStore,
) -> TypeResolution {
    resolve_module_types_from_items_with_mode(
        &item_tree.items,
        defs,
        program_defs.graph,
        program_defs,
        Some(public_surfaces),
        Some(using_scope),
        TypeResolveOptions {
            symbols: Some(symbols),
            mode: TypeResolveMode::All,
            node_store,
        },
    )
}

pub fn resolve_module_declaration_types_from_active_item_tree_with_symbols(
    item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &dyn PublicSurfaceLookup,
    using_scope: &dyn UsingScopeLookup,
    symbols: &dyn SymbolText,
) -> TypeResolution {
    let node_store = NodeStore::new();
    resolve_module_declaration_types_from_active_item_tree_with_symbols_in_store(
        item_tree,
        defs,
        program_defs,
        public_surfaces,
        using_scope,
        symbols,
        &node_store,
    )
}

pub fn resolve_module_declaration_types_from_active_item_tree_with_symbols_in_store(
    item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &dyn PublicSurfaceLookup,
    using_scope: &dyn UsingScopeLookup,
    symbols: &dyn SymbolText,
    node_store: &NodeStore,
) -> TypeResolution {
    resolve_module_types_from_items_with_mode(
        &item_tree.items,
        defs,
        program_defs.graph,
        program_defs,
        Some(public_surfaces),
        Some(using_scope),
        TypeResolveOptions {
            symbols: Some(symbols),
            mode: TypeResolveMode::Declarations,
            node_store,
        },
    )
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
