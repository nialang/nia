// SPDX-License-Identifier: GPL-3.0-or-later
use std::{collections::HashMap, sync::Arc};

use nia_ast::{Expr, ExprKind, Module, PathSegmentKind, TypeArg, TypeKind, TypeRef, Visibility};
use nia_ast_walk::{Visitor, walk_expr, walk_generic_params, walk_where_clause};
use nia_defs::{DefCollection, DefKind, PublicNamespace, PublicSurfaceLookup, UsingScopeLookup};
use nia_diagnostic::{Diagnostic, codes};
pub use nia_ids::DefId;
use nia_ids::{GlobalDefId, ModuleId};
use nia_imports::{
    ModuleGraph, ModuleGraphLookup, ModuleRootSegment, module_declaration_visibility_allows,
    visibility_allows,
};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNode, ItemTreeNodeKind, ModuleItemTree};
use nia_node_id::{NodeMap, NodeMapBuilder, NodeStore, VersionedNodeKey};
use nia_sema_ir::{BuiltinAssociatedValue, PrimitiveIntLimit, supports_primitive_int_limit};
use nia_span::Span;
use nia_symbol::{SymbolId, SymbolText, known, symbol_text_from_optional_resolver};
use nia_ty::PrimitiveTy;

#[derive(Debug, Clone, PartialEq)]
pub struct ValueResolution {
    pub node_names: NodeMap<ValueNameResolution>,
    pub node_qualified_values: NodeMap<GlobalDefId>,
    pub node_builtin_associated_values: NodeMap<BuiltinAssociatedValue>,
    /// For spans whose value resolves to an enum variant (brought in via
    /// `using` or accessed as `mod::Enum::Variant`), the parent enum's
    /// GlobalDefId so consumers can type the bare ident as that enum.
    pub node_variant_enums: NodeMap<GlobalDefId>,
    /// For `Qualified` spans like `mod::TypeName` appearing in expression
    /// position (e.g., as a type prefix in `mod::Enum::Variant` or
    /// `mod::Type::associated_fn(...)`), the resolved type's GlobalDefId.
    /// Populated by value-resolve so downstream phases can recognise these
    /// as type prefixes without re-resolving the module alias.
    pub node_qualified_type_prefixes: NodeMap<GlobalDefId>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
pub struct ValueResolutionBuilder {
    node_names: NodeMapBuilder<ValueNameResolution>,
    node_qualified_values: NodeMapBuilder<GlobalDefId>,
    node_builtin_associated_values: NodeMapBuilder<BuiltinAssociatedValue>,
    node_variant_enums: NodeMapBuilder<GlobalDefId>,
    node_qualified_type_prefixes: NodeMapBuilder<GlobalDefId>,
    diagnostics: Vec<Diagnostic>,
}

impl ValueResolution {
    pub fn with_store(store: &NodeStore) -> Self {
        Self {
            node_names: NodeMap::with_store(store),
            node_qualified_values: NodeMap::with_store(store),
            node_builtin_associated_values: NodeMap::with_store(store),
            node_variant_enums: NodeMap::with_store(store),
            node_qualified_type_prefixes: NodeMap::with_store(store),
            diagnostics: Vec::new(),
        }
    }

    pub fn builder(store: &NodeStore) -> ValueResolutionBuilder {
        ValueResolutionBuilder {
            node_names: NodeMap::builder(store),
            node_qualified_values: NodeMap::builder(store),
            node_builtin_associated_values: NodeMap::builder(store),
            node_variant_enums: NodeMap::builder(store),
            node_qualified_type_prefixes: NodeMap::builder(store),
            diagnostics: Vec::new(),
        }
    }

    pub fn into_builder(self) -> ValueResolutionBuilder {
        ValueResolutionBuilder {
            node_names: self.node_names.into_builder(),
            node_qualified_values: self.node_qualified_values.into_builder(),
            node_builtin_associated_values: self.node_builtin_associated_values.into_builder(),
            node_variant_enums: self.node_variant_enums.into_builder(),
            node_qualified_type_prefixes: self.node_qualified_type_prefixes.into_builder(),
            diagnostics: self.diagnostics,
        }
    }

    pub fn extend(self, other: Self) -> Self {
        let mut builder = self.into_builder();
        builder.extend(other);
        builder.finish()
    }
}

impl ValueResolutionBuilder {
    pub fn insert_node_name(&mut self, locator: VersionedNodeKey, resolution: ValueNameResolution) {
        self.node_names.insert(locator, resolution);
    }

    pub fn insert_node_qualified_value(&mut self, locator: VersionedNodeKey, def_id: GlobalDefId) {
        self.node_qualified_values.insert(locator, def_id);
    }

    pub fn extend(&mut self, resolution: ValueResolution) {
        self.node_names.extend_map(resolution.node_names);
        self.node_qualified_values
            .extend_map(resolution.node_qualified_values);
        self.node_builtin_associated_values
            .extend_map(resolution.node_builtin_associated_values);
        self.node_variant_enums
            .extend_map(resolution.node_variant_enums);
        self.node_qualified_type_prefixes
            .extend_map(resolution.node_qualified_type_prefixes);
        self.diagnostics.extend(resolution.diagnostics);
    }

    pub fn finish(self) -> ValueResolution {
        ValueResolution {
            node_names: self.node_names.finish(),
            node_qualified_values: self.node_qualified_values.finish(),
            node_builtin_associated_values: self.node_builtin_associated_values.finish(),
            node_variant_enums: self.node_variant_enums.finish(),
            node_qualified_type_prefixes: self.node_qualified_type_prefixes.finish(),
            diagnostics: self.diagnostics,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueNameResolution {
    Def(DefId),
    External(GlobalDefId),
    Module,
    LocalDeferred,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssociatedValueTarget {
    Nominal(GlobalDefId),
    Primitive(PrimitiveTy),
}

pub trait AssociatedValueResolver {
    fn associated_value(
        &self,
        target: AssociatedValueTarget,
        name: &SymbolId,
    ) -> Option<GlobalDefId>;
}

#[derive(Clone, Copy)]
pub struct ValueResolveOptions<'a> {
    associated_values: Option<&'a dyn AssociatedValueResolver>,
    symbols: Option<&'a dyn SymbolText>,
    node_store: &'a NodeStore,
}

impl<'a> ValueResolveOptions<'a> {
    pub fn with_store(
        associated_values: Option<&'a dyn AssociatedValueResolver>,
        symbols: Option<&'a dyn SymbolText>,
        node_store: &'a NodeStore,
    ) -> Self {
        Self {
            associated_values,
            symbols,
            node_store,
        }
    }
}

impl<F> AssociatedValueResolver for F
where
    F: Fn(AssociatedValueTarget, &SymbolId) -> Option<GlobalDefId>,
{
    fn associated_value(
        &self,
        target: AssociatedValueTarget,
        name: &SymbolId,
    ) -> Option<GlobalDefId> {
        self(target, name)
    }
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

pub fn resolve_module_values(module: &Module, defs: &DefCollection) -> ValueResolution {
    let item_tree = ModuleItemTree::from_module(module);
    resolve_module_values_from_item_tree(&item_tree, defs)
}

pub fn resolve_module_values_with_graph(
    module: &Module,
    defs: &DefCollection,
    graph: &ModuleGraph,
    program_defs: ProgramDefsContext<'_>,
) -> ValueResolution {
    let item_tree = ModuleItemTree::from_module(module);
    resolve_module_values_from_item_tree_inner(
        &item_tree,
        ValueResolveInputs {
            defs,
            graph: Some(graph),
            program_defs,
            public_surfaces: None,
            using_scope: None,
            associated_values: None,
            symbols: None,
        },
    )
}

pub fn resolve_module_values_with_symbols(
    module: &Module,
    defs: &DefCollection,
    symbols: &dyn SymbolText,
) -> ValueResolution {
    let item_tree = ModuleItemTree::from_module(module);
    resolve_module_values_from_item_tree_inner(
        &item_tree,
        ValueResolveInputs {
            defs,
            graph: None,
            program_defs: ProgramDefsContext::empty(),
            public_surfaces: None,
            using_scope: None,
            associated_values: None,
            symbols: Some(symbols),
        },
    )
}

pub fn resolve_module_values_with_context(
    module: &Module,
    defs: &DefCollection,
    graph: &ModuleGraph,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &dyn PublicSurfaceLookup,
    using_scope: &dyn UsingScopeLookup,
) -> ValueResolution {
    let item_tree = ModuleItemTree::from_module(module);
    resolve_module_values_from_item_tree_inner(
        &item_tree,
        ValueResolveInputs {
            defs,
            graph: Some(graph),
            program_defs,
            public_surfaces: Some(public_surfaces),
            using_scope: Some(using_scope),
            associated_values: None,
            symbols: None,
        },
    )
}

pub fn resolve_module_values_from_item_tree(
    item_tree: &ModuleItemTree,
    defs: &DefCollection,
) -> ValueResolution {
    resolve_module_values_from_item_tree_inner(
        item_tree,
        ValueResolveInputs {
            defs,
            graph: None,
            program_defs: ProgramDefsContext::empty(),
            public_surfaces: None,
            using_scope: None,
            associated_values: None,
            symbols: None,
        },
    )
}

pub fn resolve_module_values_from_active_item_tree(
    item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &dyn PublicSurfaceLookup,
    using_scope: &dyn UsingScopeLookup,
) -> ValueResolution {
    resolve_module_values_from_active_item_tree_with_associated_values(
        item_tree,
        defs,
        program_defs,
        public_surfaces,
        using_scope,
        None,
    )
}

pub fn resolve_module_values_from_active_item_tree_with_associated_values(
    item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &dyn PublicSurfaceLookup,
    using_scope: &dyn UsingScopeLookup,
    associated_values: Option<&dyn AssociatedValueResolver>,
) -> ValueResolution {
    resolve_module_values_from_active_item_tree_with_associated_values_and_symbols(
        item_tree,
        defs,
        program_defs,
        public_surfaces,
        using_scope,
        associated_values,
        None,
    )
}

pub fn resolve_module_values_from_active_item_tree_with_associated_values_and_symbols(
    item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &dyn PublicSurfaceLookup,
    using_scope: &dyn UsingScopeLookup,
    associated_values: Option<&dyn AssociatedValueResolver>,
    symbols: Option<&dyn SymbolText>,
) -> ValueResolution {
    let node_store = NodeStore::new();
    resolve_module_values_from_active_item_tree_with_associated_values_and_symbols_in_store(
        item_tree,
        defs,
        program_defs,
        public_surfaces,
        using_scope,
        ValueResolveOptions::with_store(associated_values, symbols, &node_store),
    )
}

pub fn resolve_module_values_from_active_item_tree_with_associated_values_and_symbols_in_store(
    item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &dyn PublicSurfaceLookup,
    using_scope: &dyn UsingScopeLookup,
    options: ValueResolveOptions<'_>,
) -> ValueResolution {
    let ValueResolveOptions {
        associated_values,
        symbols,
        node_store,
    } = options;
    resolve_module_values_from_items(
        &item_tree.items,
        ValueResolveInputs {
            defs,
            graph: program_defs.graph,
            program_defs,
            public_surfaces: Some(public_surfaces),
            using_scope: Some(using_scope),
            associated_values,
            symbols,
        },
        node_store,
    )
}

pub fn resolve_module_values_from_exprs(
    exprs: impl IntoIterator<Item = Expr>,
    defs: &DefCollection,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &dyn PublicSurfaceLookup,
    using_scope: &dyn UsingScopeLookup,
) -> ValueResolution {
    resolve_module_values_from_exprs_with_associated_values(
        exprs,
        defs,
        program_defs,
        public_surfaces,
        using_scope,
        None,
    )
}

pub fn resolve_module_values_from_exprs_with_associated_values(
    exprs: impl IntoIterator<Item = Expr>,
    defs: &DefCollection,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &dyn PublicSurfaceLookup,
    using_scope: &dyn UsingScopeLookup,
    associated_values: Option<&dyn AssociatedValueResolver>,
) -> ValueResolution {
    resolve_module_values_from_exprs_with_associated_values_and_symbols(
        exprs,
        defs,
        program_defs,
        public_surfaces,
        using_scope,
        associated_values,
        None,
    )
}

pub fn resolve_module_values_from_exprs_with_associated_values_and_symbols(
    exprs: impl IntoIterator<Item = Expr>,
    defs: &DefCollection,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &dyn PublicSurfaceLookup,
    using_scope: &dyn UsingScopeLookup,
    associated_values: Option<&dyn AssociatedValueResolver>,
    symbols: Option<&dyn SymbolText>,
) -> ValueResolution {
    let node_store = NodeStore::new();
    resolve_module_values_from_exprs_with_associated_values_and_symbols_in_store(
        exprs,
        defs,
        program_defs,
        public_surfaces,
        using_scope,
        ValueResolveOptions::with_store(associated_values, symbols, &node_store),
    )
}

pub fn resolve_module_values_from_exprs_with_associated_values_and_symbols_in_store(
    exprs: impl IntoIterator<Item = Expr>,
    defs: &DefCollection,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &dyn PublicSurfaceLookup,
    using_scope: &dyn UsingScopeLookup,
    options: ValueResolveOptions<'_>,
) -> ValueResolution {
    let ValueResolveOptions {
        associated_values,
        symbols,
        node_store,
    } = options;
    resolve_module_values_from_exprs_inner(
        exprs,
        ValueResolveInputs {
            defs,
            graph: program_defs.graph,
            program_defs,
            public_surfaces: Some(public_surfaces),
            using_scope: Some(using_scope),
            associated_values,
            symbols,
        },
        node_store,
    )
}

fn resolve_module_values_from_item_tree_inner(
    item_tree: &ModuleItemTree,
    inputs: ValueResolveInputs<'_>,
) -> ValueResolution {
    let node_store = NodeStore::new();
    resolve_module_values_from_items(&item_tree.items, inputs, &node_store)
}

fn resolve_module_values_from_exprs_inner(
    exprs: impl IntoIterator<Item = Expr>,
    inputs: ValueResolveInputs<'_>,
    node_store: &NodeStore,
) -> ValueResolution {
    let mut resolver = ValueResolver::new(inputs);
    for expr in exprs {
        resolver.visit_expr(&expr);
    }
    resolver.finish(node_store)
}

fn resolve_module_values_from_items(
    items: &[ItemTreeNode],
    inputs: ValueResolveInputs<'_>,
    node_store: &NodeStore,
) -> ValueResolution {
    let mut resolver = ValueResolver::new(inputs);
    for item in items {
        resolver.visit_item_tree_node(item);
    }
    resolver.finish(node_store)
}

struct ValueResolveInputs<'a> {
    defs: &'a DefCollection,
    graph: Option<&'a dyn ModuleGraphLookup>,
    program_defs: ProgramDefsContext<'a>,
    public_surfaces: Option<&'a dyn PublicSurfaceLookup>,
    using_scope: Option<&'a dyn UsingScopeLookup>,
    associated_values: Option<&'a dyn AssociatedValueResolver>,
    symbols: Option<&'a dyn SymbolText>,
}

struct ValueResolver<'a> {
    defs: &'a DefCollection,
    graph: Option<&'a dyn ModuleGraphLookup>,
    program_defs: ProgramDefsContext<'a>,
    public_surfaces: Option<&'a dyn PublicSurfaceLookup>,
    using_scope: Option<&'a dyn UsingScopeLookup>,
    associated_values: Option<&'a dyn AssociatedValueResolver>,
    symbols: Option<&'a dyn SymbolText>,
    node_names: HashMap<VersionedNodeKey, ValueNameResolution>,
    node_qualified_values: HashMap<VersionedNodeKey, GlobalDefId>,
    node_builtin_associated_values: HashMap<VersionedNodeKey, BuiltinAssociatedValue>,
    node_variant_enums: HashMap<VersionedNodeKey, GlobalDefId>,
    node_qualified_type_prefixes: HashMap<VersionedNodeKey, GlobalDefId>,
    diagnostics: Vec<Diagnostic>,
}

impl ValueResolver<'_> {
    fn new<'a>(inputs: ValueResolveInputs<'a>) -> ValueResolver<'a> {
        ValueResolver {
            defs: inputs.defs,
            graph: inputs.graph,
            program_defs: inputs.program_defs,
            public_surfaces: inputs.public_surfaces,
            using_scope: inputs.using_scope,
            associated_values: inputs.associated_values,
            symbols: inputs.symbols,
            node_names: HashMap::new(),
            node_qualified_values: HashMap::new(),
            node_builtin_associated_values: HashMap::new(),
            node_variant_enums: HashMap::new(),
            node_qualified_type_prefixes: HashMap::new(),
            diagnostics: Vec::new(),
        }
    }

    fn finish(self, node_store: &NodeStore) -> ValueResolution {
        let mut resolution = ValueResolution::builder(node_store);
        resolution.node_names.extend(self.node_names);
        resolution
            .node_qualified_values
            .extend(self.node_qualified_values);
        resolution
            .node_builtin_associated_values
            .extend(self.node_builtin_associated_values);
        resolution
            .node_variant_enums
            .extend(self.node_variant_enums);
        resolution
            .node_qualified_type_prefixes
            .extend(self.node_qualified_type_prefixes);
        resolution.diagnostics = self.diagnostics;
        resolution.finish()
    }

    fn graph(&self) -> Option<&dyn ModuleGraphLookup> {
        self.graph.or(self.program_defs.graph)
    }

    fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_from_optional_resolver(self.symbols, symbol)
    }

    fn qualified_path_text(&self, segments: &[PathSegment<'_>]) -> String {
        segments
            .iter()
            .map(|segment| self.path_segment_display(*segment))
            .collect::<Vec<_>>()
            .join("::")
    }

    fn path_segment_display(&self, segment: PathSegment<'_>) -> String {
        match segment.kind {
            PathSegmentKind::Name(name) => self.symbol_name(name),
            PathSegmentKind::Package => "pkg".to_string(),
            PathSegmentKind::Super => "super".to_string(),
            PathSegmentKind::SelfValue => "self".to_string(),
        }
    }

    fn visibility_allows(&self, module_id: ModuleId, visibility: Visibility) -> bool {
        if module_id == self.defs.module_id {
            return true;
        }
        let Some(graph) = self.graph() else {
            return visibility == Visibility::Public;
        };
        visibility_allows(visibility, graph, module_id, self.defs.module_id)
    }

    fn module_declaration_visible(
        &self,
        declaring_module: ModuleId,
        visibility: Visibility,
    ) -> bool {
        let Some(graph) = self.graph() else {
            return visibility == Visibility::Public;
        };
        module_declaration_visibility_allows(
            visibility,
            graph,
            declaring_module,
            self.defs.module_id,
        )
    }

    fn child_module_declaration(
        &self,
        parent_module: ModuleId,
        name: &SymbolId,
    ) -> Option<(ModuleId, Visibility)> {
        let graph = self.graph()?;
        graph.child_declaration(parent_module, name)
    }

    fn direct_type_member(&self, module_id: ModuleId, name: &SymbolId) -> DirectMember<DefId> {
        let Some(target_defs) = self.defs_for_module(module_id) else {
            return DirectMember::Unloaded;
        };
        let target_defs = target_defs.as_ref();
        let Some(def_id) = target_defs.module_scope.types.get(name) else {
            return DirectMember::Missing;
        };
        let Some(def) = target_defs.defs.get(def_id) else {
            return DirectMember::Missing;
        };
        if !self.visibility_allows(module_id, def.visibility) {
            return DirectMember::Private;
        }
        DirectMember::Visible(def_id)
    }

    fn direct_value_member(&self, module_id: ModuleId, name: &SymbolId) -> DirectMember<DefId> {
        let Some(target_defs) = self.defs_for_module(module_id) else {
            return DirectMember::Unloaded;
        };
        let target_defs = target_defs.as_ref();
        let Some(def_id) = target_defs.module_scope.values.get(name) else {
            return DirectMember::Missing;
        };
        let Some(def) = target_defs.defs.get(def_id) else {
            return DirectMember::Missing;
        };
        if !self.visibility_allows(module_id, def.visibility) {
            return DirectMember::Private;
        }
        DirectMember::Visible(def_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedNamespace {
    Module(ModuleId),
    Type(GlobalDefId),
    Primitive(PrimitiveTy),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectMember<T> {
    Visible(T),
    Private,
    Missing,
    Unloaded,
}

#[derive(Debug, Clone, Copy)]
struct PathSegment<'a> {
    kind: PathSegmentKind,
    span: Span,
    node_key: &'a VersionedNodeKey,
}

impl PathSegment<'_> {
    fn name(self) -> Option<SymbolId> {
        match self.kind {
            PathSegmentKind::Name(name) => Some(name),
            PathSegmentKind::Package | PathSegmentKind::Super | PathSegmentKind::SelfValue => None,
        }
    }
}

impl<'ast> Visitor<'ast> for ValueResolver<'_> {
    fn visit_expr(&mut self, expr: &'ast Expr) {
        match &expr.kind {
            ExprKind::Ident(name) => {
                let resolution = self.resolve_ident(name, &expr.node_key);
                if let ValueNameResolution::External(global_id) = resolution {
                    self.insert_qualified_value(&expr.node_key, global_id);
                }
                self.insert_name(&expr.node_key, resolution);
            }
            ExprKind::TypeTarget { .. } | ExprKind::TraitTarget { .. } => {
                walk_expr(self, expr);
            }
            ExprKind::BracketSuffix { callee, args } => {
                self.visit_expr(callee);
                for arg in args {
                    if let Some(expr) = &arg.expr {
                        self.visit_expr(expr);
                    }
                }
            }
            ExprKind::Qualified { lhs, .. } => {
                self.visit_expr(lhs);
                self.resolve_qualified_value(expr);
            }
            _ => walk_expr(self, expr),
        }
    }

    fn visit_type(&mut self, ty: &'ast TypeRef) {
        match &ty.kind {
            TypeKind::Path { segments } => {
                for segment in segments {
                    for arg in &segment.args {
                        match arg {
                            TypeArg::Type(ty) | TypeArg::AssocBinding { ty, .. } => {
                                self.visit_type(ty);
                            }
                            TypeArg::Const(expr) => self.visit_expr(expr),
                            TypeArg::TypeOrConst { ty, .. } => self.visit_type_candidate(ty),
                        }
                    }
                }
            }
            _ => nia_ast_walk::walk_type(self, ty),
        }
    }
}

impl<'a> ValueResolver<'a> {
    fn visit_type_candidate(&mut self, ty: &TypeRef) {
        if let TypeKind::Path { segments } = &ty.kind {
            for segment in segments {
                for arg in &segment.args {
                    match arg {
                        TypeArg::Type(ty) | TypeArg::AssocBinding { ty, .. } => {
                            self.visit_type(ty);
                        }
                        TypeArg::Const(expr) => self.visit_expr(expr),
                        TypeArg::TypeOrConst { ty, .. } => self.visit_type_candidate(ty),
                    }
                }
            }
        }
    }

    fn visit_item_tree_node(&mut self, item: &ItemTreeNode) {
        match &item.kind {
            ItemTreeNodeKind::Struct(item_struct) => {
                walk_generic_params(self, &item_struct.generics);
                walk_where_clause(self, &item_struct.where_clause);
                for field in &item_struct.fields {
                    self.visit_type(&field.ty);
                }
            }
            ItemTreeNodeKind::Union(item_union) => {
                walk_generic_params(self, &item_union.generics);
                walk_where_clause(self, &item_union.where_clause);
                for field in &item_union.fields {
                    self.visit_type(&field.ty);
                }
            }
            ItemTreeNodeKind::Trait(item_trait) => {
                walk_generic_params(self, &item_trait.generics);
                for supertrait in &item_trait.supertraits {
                    self.visit_type(supertrait);
                }
                walk_where_clause(self, &item_trait.where_clause);
                for method in &item_trait.methods {
                    self.visit_function(&method.function);
                }
            }
            ItemTreeNodeKind::Extend(extend) => {
                walk_generic_params(self, &extend.generics);
                self.visit_type(&extend.target);
                if let Some(trait_ref) = &extend.trait_ref {
                    self.visit_type(trait_ref);
                }
                walk_where_clause(self, &extend.where_clause);
                for associated_type in &extend.associated_types {
                    self.visit_type(&associated_type.ty);
                }
                for associated_value in &extend.associated_values {
                    if let Some(ty) = &associated_value.binding.ty {
                        self.visit_type(ty);
                    }
                    if let Some(value) = &associated_value.binding.value {
                        self.visit_expr(value);
                    }
                }
                for method in &extend.methods {
                    self.visit_function(&method.function);
                }
            }
            ItemTreeNodeKind::Enum(item_enum) => {
                if let Some(backing_type) = &item_enum.backing_type {
                    self.visit_type(backing_type);
                }
                for variant in &item_enum.variants {
                    match &variant.payload {
                        nia_ast::EnumVariantPayload::Unit => {}
                        nia_ast::EnumVariantPayload::Tuple(fields) => {
                            for field in fields {
                                self.visit_type(field);
                            }
                        }
                        nia_ast::EnumVariantPayload::Named(fields) => {
                            for field in fields {
                                self.visit_type(&field.ty);
                            }
                        }
                    }
                    if let Some(value) = &variant.value {
                        self.visit_expr(value);
                    }
                }
            }
            ItemTreeNodeKind::Binding(binding) => {
                if let Some(ty) = &binding.ty {
                    self.visit_type(ty);
                }
                if let Some(value) = &binding.value {
                    self.visit_expr(value);
                }
            }
            ItemTreeNodeKind::Function(function) => self.visit_function(function),
            ItemTreeNodeKind::Module(_) | ItemTreeNodeKind::Using(_) => {}
            ItemTreeNodeKind::TypeAlias(alias) => {
                walk_generic_params(self, &alias.generics);
                walk_where_clause(self, &alias.where_clause);
                if let Some(ty) = &alias.ty {
                    self.visit_type(ty);
                }
            }
        }
    }

    fn resolve_qualified_value(&mut self, expr: &Expr) {
        let Some(segments) = qualified_path_segments(expr) else {
            return;
        };
        if segments.len() < 2 {
            return;
        };
        let path_text = self.qualified_path_text(&segments);
        let prefix = &segments[..segments.len() - 1];
        let final_segment = segments[segments.len() - 1];
        let Some(namespace) = self.resolve_namespace_path(prefix) else {
            return;
        };
        match namespace {
            ResolvedNamespace::Module(module_id) => {
                self.resolve_module_qualified_value(
                    expr.span,
                    &expr.node_key,
                    module_id,
                    final_segment,
                    &path_text,
                );
            }
            ResolvedNamespace::Type(type_id) => {
                self.resolve_type_qualified_value(&expr.node_key, type_id, final_segment);
            }
            ResolvedNamespace::Primitive(primitive) => {
                self.resolve_primitive_qualified_value(&expr.node_key, primitive, final_segment);
            }
        }
    }

    fn resolve_namespace_path(
        &mut self,
        segments: &[PathSegment<'_>],
    ) -> Option<ResolvedNamespace> {
        // Expression-qualified paths are also how enum variants and associated
        // functions surface. Resolve every prefix as either a module namespace
        // or a type namespace, then let downstream phases use
        // `qualified_type_prefixes` to avoid reinterpreting the same spans.
        let first = *segments.first()?;
        let mut namespace = self.resolve_root_namespace(first)?;
        for segment in &segments[1..] {
            namespace = self.resolve_child_namespace(namespace, *segment)?;
        }
        Some(namespace)
    }

    fn resolve_root_namespace(&mut self, segment: PathSegment<'_>) -> Option<ResolvedNamespace> {
        if let Some(module_id) = self.root_module_for_segment(segment) {
            self.insert_name(segment.node_key, ValueNameResolution::Module);
            return Some(ResolvedNamespace::Module(module_id));
        }
        let name = segment.name()?;
        if let Some(scope) = self.using_scope
            && let Some(module_id) = scope.using_module(&name)
        {
            self.insert_name(segment.node_key, ValueNameResolution::Module);
            return Some(ResolvedNamespace::Module(module_id));
        }
        if let Some(def_id) = self.defs.module_scope.types.get(&name) {
            let type_id = GlobalDefId {
                module_id: self.defs.module_id,
                def_id,
            };
            self.insert_qualified_type_prefix(segment.node_key, type_id);
            return Some(ResolvedNamespace::Type(type_id));
        }
        if let Some(scope) = self.using_scope
            && let Some(entry) = scope.using_type(&name)
        {
            let type_id = GlobalDefId {
                module_id: entry.target_module,
                def_id: entry.target_def_id,
            };
            self.insert_name(segment.node_key, ValueNameResolution::External(type_id));
            self.insert_qualified_type_prefix(segment.node_key, type_id);
            return Some(ResolvedNamespace::Type(type_id));
        }
        if let Some(primitive) = primitive_for_symbol(name) {
            return Some(ResolvedNamespace::Primitive(primitive));
        }
        None
    }

    fn root_module_for_segment(&self, segment: PathSegment<'_>) -> Option<ModuleId> {
        let graph = self.graph()?;
        graph.root_module_for_segment(
            self.defs.module_id,
            module_root_segment_from_path_segment(segment.kind),
        )
    }

    fn resolve_child_namespace(
        &mut self,
        namespace: ResolvedNamespace,
        segment: PathSegment<'_>,
    ) -> Option<ResolvedNamespace> {
        match namespace {
            ResolvedNamespace::Module(module_id) => {
                let Some(name) = segment.name() else {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::NAME_RESOLUTION,
                        segment.span,
                        format!(
                            "expected namespace name, found `{}`",
                            self.path_segment_display(segment)
                        ),
                    ));
                    return None;
                };
                if let Some(surfaces) = self.public_surfaces {
                    if let Some(child_module) = surfaces.public_module(module_id, &name) {
                        return Some(ResolvedNamespace::Module(child_module));
                    }
                    if let Some(item) = surfaces.public_type(module_id, &name) {
                        return Some(ResolvedNamespace::Type(GlobalDefId {
                            module_id: item.target_module,
                            def_id: item.target_def_id,
                        }));
                    }
                }
                if let Some((child_module, visibility)) =
                    self.child_module_declaration(module_id, &name)
                {
                    if self.module_declaration_visible(module_id, visibility) {
                        return Some(ResolvedNamespace::Module(child_module));
                    }
                    let name = self.symbol_name(name);
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::NAME_RESOLUTION,
                        segment.span,
                        format!("module namespace `{}` is private", name),
                    ));
                    return None;
                }
                match self.direct_type_member(module_id, &name) {
                    DirectMember::Visible(def_id) => {
                        Some(ResolvedNamespace::Type(GlobalDefId { module_id, def_id }))
                    }
                    DirectMember::Private => {
                        let name = self.symbol_name(name);
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::NAME_RESOLUTION,
                            segment.span,
                            format!("type `{}` is private", name),
                        ));
                        None
                    }
                    DirectMember::Missing => {
                        let name = self.symbol_name(name);
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::NAME_RESOLUTION,
                            segment.span,
                            format!("unknown namespace `{}`", name),
                        ));
                        None
                    }
                    DirectMember::Unloaded => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::NAME_RESOLUTION,
                            segment.span,
                            "module namespace refers to an unloaded module",
                        ));
                        None
                    }
                }
            }
            ResolvedNamespace::Type(_) | ResolvedNamespace::Primitive(_) => None,
        }
    }

    fn resolve_module_qualified_value(
        &mut self,
        span: Span,
        node_key: &VersionedNodeKey,
        module_id: ModuleId,
        name: PathSegment<'_>,
        path_text: &str,
    ) {
        let Some(symbol) = name.name() else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                span,
                format!(
                    "expected value name, found `{}`",
                    self.path_segment_display(name)
                ),
            ));
            return;
        };
        if let Some(surfaces) = self.public_surfaces {
            if let Some(item) = surfaces.public_value(module_id, &symbol) {
                self.insert_qualified_value(
                    node_key,
                    GlobalDefId {
                        module_id: item.target_module,
                        def_id: item.target_def_id,
                    },
                );
                if let Some(enum_id) = item.parent_enum {
                    self.insert_variant_enum(node_key, enum_id);
                }
                return;
            }
            if let Some(item) = surfaces.public_type(module_id, &symbol) {
                self.insert_qualified_type_prefix(
                    node_key,
                    GlobalDefId {
                        module_id: item.target_module,
                        def_id: item.target_def_id,
                    },
                );
                return;
            }
            if surfaces.public_module(module_id, &symbol).is_some() {
                return;
            }
        }
        if let Some((_child_module, visibility)) = self.child_module_declaration(module_id, &symbol)
        {
            if self.module_declaration_visible(module_id, visibility) {
                return;
            }
            let symbol = self.symbol_name(symbol);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                span,
                format!("module namespace `{}` is private", symbol),
            ));
            return;
        }
        match self.direct_type_member(module_id, &symbol) {
            DirectMember::Visible(def_id) => {
                self.insert_qualified_type_prefix(node_key, GlobalDefId { module_id, def_id });
                return;
            }
            DirectMember::Private => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::NAME_RESOLUTION,
                    span,
                    format!("type `{path_text}` is private"),
                ));
                return;
            }
            DirectMember::Missing => {}
            DirectMember::Unloaded => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::NAME_RESOLUTION,
                    span,
                    "module namespace refers to an unloaded module",
                ));
                return;
            }
        }
        let def_id = match self.direct_value_member(module_id, &symbol) {
            DirectMember::Visible(def_id) => def_id,
            DirectMember::Private => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::NAME_RESOLUTION,
                    span,
                    format!("value `{path_text}` is private"),
                ));
                return;
            }
            DirectMember::Missing => {
                let symbol = self.symbol_name(symbol);
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::NAME_RESOLUTION,
                    span,
                    format!("unknown value `{}`", symbol),
                ));
                return;
            }
            DirectMember::Unloaded => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::NAME_RESOLUTION,
                    span,
                    "module namespace refers to an unloaded module",
                ));
                return;
            }
        };
        let Some(target_defs) = self.defs_for_module(module_id) else {
            return;
        };
        let target_defs = target_defs.as_ref();
        let Some(def) = target_defs.defs.get(def_id) else {
            return;
        };
        if matches!(
            def.kind,
            DefKind::Function | DefKind::Global | DefKind::Const
        ) {
            self.insert_qualified_value(node_key, GlobalDefId { module_id, def_id });
        }
    }

    fn resolve_type_qualified_value(
        &mut self,
        node_key: &VersionedNodeKey,
        type_id: GlobalDefId,
        name: PathSegment<'_>,
    ) {
        let Some(symbol) = name.name() else {
            return;
        };
        let Some(target_defs) = self.defs_for_module(type_id.module_id) else {
            return;
        };
        let target_defs = target_defs.as_ref();
        let Some(def) = target_defs.defs.get(type_id.def_id) else {
            return;
        };
        if def.kind == DefKind::Enum
            && let Some(enum_scope) = target_defs.scopes.enum_members.get(&type_id.def_id)
            && let Some(variant_def_id) = enum_scope.variants.get(&symbol)
        {
            let variant_id = GlobalDefId {
                module_id: type_id.module_id,
                def_id: variant_def_id,
            };
            self.insert_qualified_value(node_key, variant_id);
            self.insert_variant_enum(node_key, type_id);
            return;
        }
        self.resolve_associated_value(node_key, AssociatedValueTarget::Nominal(type_id), &symbol);
    }

    fn resolve_primitive_qualified_value(
        &mut self,
        node_key: &VersionedNodeKey,
        primitive: PrimitiveTy,
        name: PathSegment<'_>,
    ) {
        let Some(symbol) = name.name() else {
            return;
        };
        if let Some(value) = primitive_associated_value(primitive, symbol) {
            self.insert_builtin_associated_value(node_key, value);
            return;
        }
        self.resolve_associated_value(
            node_key,
            AssociatedValueTarget::Primitive(primitive),
            &symbol,
        );
    }

    fn resolve_associated_value(
        &mut self,
        node_key: &VersionedNodeKey,
        target: AssociatedValueTarget,
        name: &SymbolId,
    ) {
        if let Some(resolver) = self.associated_values
            && let Some(def_id) = resolver.associated_value(target, name)
        {
            self.insert_qualified_value(node_key, def_id);
        }
    }

    fn resolve_ident(
        &mut self,
        name: &SymbolId,
        node_key: &VersionedNodeKey,
    ) -> ValueNameResolution {
        if let Some(def_id) = self.defs.module_scope.values.get(name) {
            let Some(def) = self.defs.defs.get(def_id) else {
                return ValueNameResolution::Error;
            };
            if matches!(
                def.kind,
                DefKind::Function | DefKind::Global | DefKind::Const
            ) {
                return ValueNameResolution::Def(def_id);
            }
        }

        if let Some(scope) = self.using_scope
            && let Some(entry) = scope.using_value(name)
            && entry.namespace == PublicNamespace::Value
        {
            if let Some(enum_id) = entry.parent_enum {
                self.insert_variant_enum(node_key, enum_id);
            }
            return ValueNameResolution::External(GlobalDefId {
                module_id: entry.target_module,
                def_id: entry.target_def_id,
            });
        }

        if let Some(def_id) = self.defs.module_scope.types.get(name) {
            self.insert_qualified_type_prefix(
                node_key,
                GlobalDefId {
                    module_id: self.defs.module_id,
                    def_id,
                },
            );
        }
        if let Some(scope) = self.using_scope
            && let Some(entry) = scope.using_type(name)
            && entry.namespace == PublicNamespace::Type
        {
            self.insert_qualified_type_prefix(
                node_key,
                GlobalDefId {
                    module_id: entry.target_module,
                    def_id: entry.target_def_id,
                },
            );
        }
        if self
            .using_scope
            .is_some_and(|scope| scope.has_unresolved_using_name(name))
        {
            return ValueNameResolution::Error;
        }

        // Local bindings and parameters are resolved by nia-local-resolve.
        ValueNameResolution::LocalDeferred
    }

    fn defs_for_module(&self, module_id: ModuleId) -> Option<ModuleDefs<'_>> {
        if module_id == self.defs.module_id {
            Some(ModuleDefs::Borrowed(self.defs))
        } else {
            Some(ModuleDefs::Shared((self.program_defs.defs?)(module_id)?))
        }
    }

    fn insert_name(&mut self, node_key: &VersionedNodeKey, resolution: ValueNameResolution) {
        self.node_names.insert(node_key.clone(), resolution);
    }

    fn insert_qualified_value(&mut self, node_key: &VersionedNodeKey, global_id: GlobalDefId) {
        self.node_qualified_values
            .insert(node_key.clone(), global_id);
    }

    fn insert_builtin_associated_value(
        &mut self,
        node_key: &VersionedNodeKey,
        value: BuiltinAssociatedValue,
    ) {
        self.node_builtin_associated_values
            .insert(node_key.clone(), value);
    }

    fn insert_variant_enum(&mut self, node_key: &VersionedNodeKey, enum_id: GlobalDefId) {
        self.node_variant_enums.insert(node_key.clone(), enum_id);
    }

    fn insert_qualified_type_prefix(&mut self, node_key: &VersionedNodeKey, type_id: GlobalDefId) {
        self.node_qualified_type_prefixes
            .insert(node_key.clone(), type_id);
    }
}

fn qualified_path_segments(expr: &Expr) -> Option<Vec<PathSegment<'_>>> {
    fn collect<'a>(expr: &'a Expr, segments: &mut Vec<PathSegment<'a>>) -> Option<()> {
        match &expr.kind {
            ExprKind::Ident(name) => {
                segments.push(PathSegment {
                    kind: PathSegmentKind::Name(*name),
                    span: expr.span,
                    node_key: &expr.node_key,
                });
                Some(())
            }
            ExprKind::SelfValue => {
                segments.push(PathSegment {
                    kind: PathSegmentKind::SelfValue,
                    span: expr.span,
                    node_key: &expr.node_key,
                });
                Some(())
            }
            ExprKind::PathRoot(kind) => {
                segments.push(PathSegment {
                    kind: *kind,
                    span: expr.span,
                    node_key: &expr.node_key,
                });
                Some(())
            }
            ExprKind::Qualified { lhs, name } => {
                collect(lhs, segments)?;
                segments.push(PathSegment {
                    kind: PathSegmentKind::Name(*name),
                    span: expr.span,
                    node_key: &expr.node_key,
                });
                Some(())
            }
            _ => None,
        }
    }

    let mut segments = Vec::new();
    collect(expr, &mut segments)?;
    Some(segments)
}

fn primitive_associated_value(
    primitive: PrimitiveTy,
    name: SymbolId,
) -> Option<BuiltinAssociatedValue> {
    let kind = match name {
        known::MIN => PrimitiveIntLimit::Min,
        known::MAX => PrimitiveIntLimit::Max,
        _ => return None,
    };
    supports_primitive_int_limit(primitive)
        .then_some(BuiltinAssociatedValue::PrimitiveIntLimit { primitive, kind })
}

fn module_root_segment_from_path_segment(kind: PathSegmentKind) -> ModuleRootSegment {
    match kind {
        PathSegmentKind::SelfValue => ModuleRootSegment::Current,
        PathSegmentKind::Super => ModuleRootSegment::Parent,
        PathSegmentKind::Package => ModuleRootSegment::PackageRelative,
        PathSegmentKind::Name(name) => ModuleRootSegment::Named(name),
    }
}

fn primitive_for_symbol(name: SymbolId) -> Option<PrimitiveTy> {
    Some(match name {
        known::I8 => PrimitiveTy::I8,
        known::I16 => PrimitiveTy::I16,
        known::I32 => PrimitiveTy::I32,
        known::I64 => PrimitiveTy::I64,
        known::I128 => PrimitiveTy::I128,
        known::ISIZE => PrimitiveTy::Isize,
        known::U8 => PrimitiveTy::U8,
        known::U16 => PrimitiveTy::U16,
        known::U32 => PrimitiveTy::U32,
        known::U64 => PrimitiveTy::U64,
        known::U128 => PrimitiveTy::U128,
        known::USIZE => PrimitiveTy::Usize,
        known::F32 => PrimitiveTy::F32,
        known::F64 => PrimitiveTy::F64,
        known::BOOL => PrimitiveTy::Bool,
        known::CHAR => PrimitiveTy::Char,
        known::NEVER => PrimitiveTy::Never,
        _ => return None,
    })
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
