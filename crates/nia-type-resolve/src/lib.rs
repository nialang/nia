// SPDX-License-Identifier: GPL-3.0-or-later
//! Resolves source type references to semantic definitions and generic names.
//!
//! The resolver records node-local identities and diagnostics while keeping
//! module/import visibility decisions in the supplied program context.
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
/// The complete type-name and const-generic resolution product for a module.
pub struct TypeResolution {
    /// Type-name resolutions keyed by source node site.
    pub node_type_names: HashMap<NodeSite, TypeNameResolution>,
    /// Qualified type references resolved to global definitions.
    pub node_qualified_type_names: HashMap<NodeSite, GlobalDefId>,
    /// Const-generic parameter names keyed by versioned node identity.
    pub node_const_generic_names: NodeMap<SymbolId>,
    /// Diagnostics emitted while resolving the module.
    pub diagnostics: Vec<Diagnostic>,
}

/// Semantic category recorded for a source type name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeNameResolution {
    /// A primitive type spelling.
    Primitive(PrimitiveTypeSpelling),
    /// A builtin trait.
    BuiltinTrait(BuiltinTrait),
    /// A definition local to the resolved module.
    Def(DefId),
    /// A definition owned by another module.
    External(GlobalDefId),
    /// A generic type parameter.
    GenericParam,
    /// An associated type projection.
    AssociatedType,
    /// An unresolved or invalid type name.
    Error,
}

/// Optional program-wide definition and module-graph providers.
#[derive(Clone, Copy)]
pub struct ProgramDefsContext<'a> {
    /// Resolves a module id to its definitions.
    pub defs: Option<&'a dyn Fn(ModuleId) -> Option<Arc<DefCollection>>>,
    /// Provides module graph and import visibility information.
    pub graph: Option<&'a dyn ModuleGraphLookup>,
}

impl<'a> ProgramDefsContext<'a> {
    /// Creates a context without program-wide providers.
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

/// Resolves all type references in a parsed module.
pub fn resolve_module_types(module: &Module, defs: &DefCollection) -> TypeResolution {
    let item_tree = ModuleItemTree::from_module(module);
    resolve_module_types_from_item_tree(&item_tree, defs)
}

/// Resolves module types while retaining symbol text for diagnostics.
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

/// Resolves module types with cross-module graph and definition providers.
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

/// Resolves module types with graph, public-surface, and using-scope context.
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

/// Resolves types from an already lowered module item tree.
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

/// Resolves all active items, including function bodies, in an item tree.
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

/// Resolves only active declarations, excluding function-body type references.
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

/// Resolves active items while using caller-provided symbol text.
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

/// Resolves active items into a caller-owned versioned node store.
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

/// Resolves active declarations while using caller-provided symbol text.
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

/// Resolves active declarations into a caller-owned versioned node store.
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
