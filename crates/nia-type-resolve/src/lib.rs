// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{
    ArrayLen, AssocBindingKey, FunctionItem, GenericParam, GenericParamKind, Item, ItemKind,
    Module, PathSegmentKind, TypeArg, TypeKind, TypePathSegment, TypeRef,
};
use nia_ast_walk::{Visitor, walk_function, walk_item};
use nia_defs::{DefCollection, DefKind, ModuleUsingScope, PublicNamespace, PublicSurfaces};
use nia_diagnostic::{Diagnostic, codes};
pub use nia_ids::DefId;
use nia_ids::{GlobalDefId, ModuleId, Visibility};
use nia_imports::{
    ModuleGraph, ModuleRootSegment, module_declaration_visibility_allows, visibility_allows,
};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNode, ItemTreeNodeKind, ModuleItemTree};
use nia_node_id::{NodeSite, VersionedNodeKey};
use nia_span::Span;
use nia_symbol::{SymbolId, SymbolText, known, symbol_text_from_optional_resolver};
use nia_ty::{BuiltinTrait, PrimitiveTy, PrimitiveTypeSpelling};

#[derive(Debug, Clone, PartialEq)]
pub struct TypeResolution {
    pub node_type_names: HashMap<NodeSite, TypeNameResolution>,
    pub node_qualified_type_names: HashMap<NodeSite, GlobalDefId>,
    pub node_const_generic_names: HashMap<VersionedNodeKey, SymbolId>,
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
    pub defs: Option<&'a dyn Fn(ModuleId) -> Option<DefCollection>>,
    pub graph: Option<&'a ModuleGraph>,
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
    Owned(DefCollection),
}

impl ModuleDefs<'_> {
    fn as_ref(&self) -> &DefCollection {
        match self {
            ModuleDefs::Borrowed(defs) => defs,
            ModuleDefs::Owned(defs) => defs,
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
    public_surfaces: &PublicSurfaces,
    using_scope: &ModuleUsingScope,
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
    public_surfaces: &PublicSurfaces,
    using_scope: &ModuleUsingScope,
) -> TypeResolution {
    resolve_module_types_from_items(
        &item_tree.items,
        defs,
        program_defs.graph,
        program_defs,
        Some(public_surfaces),
        Some(using_scope),
        None,
    )
}

pub fn resolve_module_declaration_types_from_active_item_tree(
    item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &PublicSurfaces,
    using_scope: &ModuleUsingScope,
) -> TypeResolution {
    resolve_module_types_from_items_with_mode(
        &item_tree.items,
        defs,
        program_defs.graph,
        program_defs,
        Some(public_surfaces),
        Some(using_scope),
        None,
        TypeResolveMode::Declarations,
    )
}

pub fn resolve_module_types_from_active_item_tree_with_symbols(
    item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &PublicSurfaces,
    using_scope: &ModuleUsingScope,
    symbols: &dyn SymbolText,
) -> TypeResolution {
    resolve_module_types_from_items(
        &item_tree.items,
        defs,
        program_defs.graph,
        program_defs,
        Some(public_surfaces),
        Some(using_scope),
        Some(symbols),
    )
}

pub fn resolve_module_declaration_types_from_active_item_tree_with_symbols(
    item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &PublicSurfaces,
    using_scope: &ModuleUsingScope,
    symbols: &dyn SymbolText,
) -> TypeResolution {
    resolve_module_types_from_items_with_mode(
        &item_tree.items,
        defs,
        program_defs.graph,
        program_defs,
        Some(public_surfaces),
        Some(using_scope),
        Some(symbols),
        TypeResolveMode::Declarations,
    )
}

fn resolve_module_types_from_item_tree_inner(
    item_tree: &ModuleItemTree,
    defs: &DefCollection,
    graph: Option<&ModuleGraph>,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: Option<&PublicSurfaces>,
    using_scope: Option<&ModuleUsingScope>,
    symbols: Option<&dyn SymbolText>,
) -> TypeResolution {
    resolve_module_types_from_items_with_mode(
        &item_tree.items,
        defs,
        graph,
        program_defs,
        public_surfaces,
        using_scope,
        symbols,
        TypeResolveMode::All,
    )
}

fn resolve_module_types_from_items(
    items: &[ItemTreeNode],
    defs: &DefCollection,
    graph: Option<&ModuleGraph>,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: Option<&PublicSurfaces>,
    using_scope: Option<&ModuleUsingScope>,
    symbols: Option<&dyn SymbolText>,
) -> TypeResolution {
    resolve_module_types_from_items_with_mode(
        items,
        defs,
        graph,
        program_defs,
        public_surfaces,
        using_scope,
        symbols,
        TypeResolveMode::All,
    )
}

fn resolve_module_types_from_items_with_mode(
    items: &[ItemTreeNode],
    defs: &DefCollection,
    graph: Option<&ModuleGraph>,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: Option<&PublicSurfaces>,
    using_scope: Option<&ModuleUsingScope>,
    symbols: Option<&dyn SymbolText>,
    mode: TypeResolveMode,
) -> TypeResolution {
    let mut resolver = TypeResolver {
        defs,
        graph,
        program_defs,
        public_surfaces,
        using_scope,
        symbols,
        node_type_names: HashMap::new(),
        node_qualified_type_names: HashMap::new(),
        node_const_generic_names: HashMap::new(),
        diagnostics: Vec::new(),
        generic_stack: Vec::new(),
        self_type_stack: Vec::new(),
        associated_type_stack: Vec::new(),
        mode,
    };
    for item in items {
        resolver.visit_item_tree_node(item);
    }
    TypeResolution {
        node_type_names: resolver.node_type_names,
        node_qualified_type_names: resolver.node_qualified_type_names,
        node_const_generic_names: resolver.node_const_generic_names,
        diagnostics: resolver.diagnostics,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeResolveMode {
    All,
    Declarations,
}

struct TypeResolver<'a> {
    defs: &'a DefCollection,
    graph: Option<&'a ModuleGraph>,
    program_defs: ProgramDefsContext<'a>,
    public_surfaces: Option<&'a PublicSurfaces>,
    using_scope: Option<&'a ModuleUsingScope>,
    symbols: Option<&'a dyn SymbolText>,
    node_type_names: HashMap<NodeSite, TypeNameResolution>,
    node_qualified_type_names: HashMap<NodeSite, GlobalDefId>,
    node_const_generic_names: HashMap<VersionedNodeKey, SymbolId>,
    diagnostics: Vec<Diagnostic>,
    generic_stack: Vec<Vec<GenericParam>>,
    self_type_stack: Vec<Span>,
    associated_type_stack: Vec<Vec<SymbolId>>,
    mode: TypeResolveMode,
}

impl TypeResolver<'_> {
    fn primitive_type_spelling_for_symbol(&self, name: &SymbolId) -> Option<PrimitiveTypeSpelling> {
        primitive_type_spelling_for_known_symbol(name).or_else(|| {
            let text = self.symbols?.symbol_text(*name)?;
            PrimitiveTypeSpelling::from_name(&text)
        })
    }

    fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_from_optional_resolver(self.symbols, symbol)
    }

    fn type_segment_display(&self, segment: &TypePathSegment) -> String {
        match segment.kind {
            PathSegmentKind::Name(name) => self.symbol_name(name),
            PathSegmentKind::Package => "pkg".to_string(),
            PathSegmentKind::Super => "super".to_string(),
            PathSegmentKind::SelfValue => "self".to_string(),
        }
    }

    fn type_path_text(&self, segments: &[TypePathSegment]) -> String {
        segments
            .iter()
            .map(|segment| self.type_segment_display(segment))
            .collect::<Vec<_>>()
            .join("::")
    }

    fn graph(&self) -> Option<&ModuleGraph> {
        self.graph.or(self.program_defs.graph)
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
        let parent = graph.get(parent_module)?;
        let target = parent.children.get(name).copied()?;
        let declaration = parent
            .declarations
            .iter()
            .find(|declaration| &declaration.name == name && declaration.target == target)?;
        Some((target, declaration.visibility))
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedNamespace {
    Module(ModuleId),
    Type(GlobalDefId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectMember<T> {
    Visible(T),
    Private,
    Missing,
    Unloaded,
}

impl<'ast> Visitor<'ast> for TypeResolver<'_> {
    fn visit_item(&mut self, item: &'ast Item) {
        match &item.kind {
            ItemKind::Struct(item_struct) => {
                self.with_generics(&item_struct.generics, |resolver| walk_item(resolver, item));
            }
            ItemKind::Union(item_union) => {
                self.with_generics(&item_union.generics, |resolver| walk_item(resolver, item));
            }
            ItemKind::Trait(item_trait) => {
                self.with_generics(&item_trait.generics, |resolver| {
                    let associated_types = item_trait
                        .associated_types
                        .iter()
                        .map(|associated_type| associated_type.name.clone())
                        .collect::<Vec<_>>();
                    resolver.with_self_type(item.span, |resolver| {
                        resolver.with_associated_types(associated_types, |resolver| {
                            walk_item(resolver, item)
                        });
                    });
                });
            }
            ItemKind::Extend(extend) => {
                self.with_generics(&extend.generics, |resolver| {
                    let associated_types = extend
                        .associated_types
                        .iter()
                        .map(|associated_type| associated_type.name.clone())
                        .collect::<Vec<_>>();
                    resolver.with_self_type(extend.target.span, |resolver| {
                        resolver.visit_type(&extend.target);
                        if let Some(trait_ref) = &extend.trait_ref {
                            resolver.visit_type(trait_ref);
                        }
                        resolver.visit_where_clause(&extend.where_clause);
                        for associated_type in &extend.associated_types {
                            resolver.visit_type(&associated_type.ty);
                        }
                        for associated_value in &extend.associated_values {
                            if let Some(ty) = &associated_value.binding.ty {
                                resolver.visit_type(ty);
                            }
                            if resolver.mode == TypeResolveMode::All
                                && let Some(value) = &associated_value.binding.value
                            {
                                resolver.visit_expr(value);
                            }
                        }
                        resolver.with_associated_types(associated_types, |resolver| {
                            for method in &extend.methods {
                                resolver.visit_function(&method.function);
                            }
                        });
                    });
                });
            }
            ItemKind::TypeAlias(alias) => {
                self.with_generics(&alias.generics, |resolver| walk_item(resolver, item));
            }
            _ => walk_item(self, item),
        }
    }

    fn visit_function(&mut self, function: &'ast FunctionItem) {
        self.with_generics(&function.generics, |resolver| {
            walk_function(resolver, function);
        });
    }

    fn visit_expr(&mut self, expr: &'ast nia_ast::Expr) {
        match &expr.kind {
            nia_ast::ExprKind::Ident(name) => {
                if self.is_comptime_generic_param(name) {
                    self.node_const_generic_names
                        .insert(expr.node_key.clone(), name.clone());
                }
            }
            nia_ast::ExprKind::BracketSuffix { callee, args } => {
                self.visit_expr(callee);
                for arg in args {
                    if let Some(expr) = &arg.expr {
                        nia_ast_walk::walk_expr(self, expr);
                    }
                    if let Some(ty) = &arg.ty {
                        if arg.expr.is_some() {
                            self.resolve_type_candidate(ty);
                        } else {
                            self.visit_type(ty);
                        }
                    }
                }
            }
            _ => nia_ast_walk::walk_expr(self, expr),
        }
    }

    fn visit_type(&mut self, ty: &'ast TypeRef) {
        match &ty.kind {
            TypeKind::Error | TypeKind::Infer | TypeKind::Void | TypeKind::Never => {}
            TypeKind::Projection { ty, trait_ref, .. } => {
                self.visit_type(ty);
                self.visit_type(trait_ref);
            }
            TypeKind::SelfType => {
                if self.self_type_stack.is_empty() {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::NAME_RESOLUTION,
                        ty.span,
                        "`Self` is only valid in traits and extend blocks",
                    ));
                }
            }
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePointer { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::SlicePointee { elem } => {
                self.visit_type(elem);
            }
            TypeKind::Array { len, elem } => {
                if let ArrayLen::Expr(expr) = len {
                    nia_ast_walk::walk_expr(self, expr);
                }
                self.visit_type(elem);
            }
            TypeKind::Range { start, end, .. } => {
                if let Some(start) = start {
                    self.visit_type(start);
                }
                if let Some(end) = end {
                    self.visit_type(end);
                }
            }
            TypeKind::FunctionPointer {
                params,
                return_type,
                ..
            } => {
                for param in params {
                    self.visit_type(param);
                }
                if let Some(return_type) = return_type {
                    self.visit_type(return_type);
                }
            }
            TypeKind::Optional { elem } => self.visit_type(elem),
            TypeKind::ErrorUnion { error, value } => {
                self.visit_type(error);
                self.visit_type(value);
            }
            TypeKind::Path { segments } => self.resolve_type_path(ty, segments),
        }
    }
}

impl TypeResolver<'_> {
    fn visit_item_tree_node(&mut self, item: &ItemTreeNode) {
        match &item.kind {
            ItemTreeNodeKind::Struct(item_struct) => {
                self.with_generics(&item_struct.generics, |resolver| {
                    resolver.visit_where_clause(&item_struct.where_clause);
                    for field in &item_struct.fields {
                        resolver.visit_type(&field.ty);
                    }
                });
            }
            ItemTreeNodeKind::Union(item_union) => {
                self.with_generics(&item_union.generics, |resolver| {
                    resolver.visit_where_clause(&item_union.where_clause);
                    for field in &item_union.fields {
                        resolver.visit_type(&field.ty);
                    }
                });
            }
            ItemTreeNodeKind::Trait(item_trait) => {
                self.with_generics(&item_trait.generics, |resolver| {
                    let associated_types = item_trait
                        .associated_types
                        .iter()
                        .map(|associated_type| associated_type.name.clone())
                        .collect::<Vec<_>>();
                    resolver.with_self_type(item.span, |resolver| {
                        resolver.with_associated_types(associated_types, |resolver| {
                            for supertrait in &item_trait.supertraits {
                                resolver.visit_type(supertrait);
                            }
                            resolver.visit_where_clause(&item_trait.where_clause);
                            for associated_value in &item_trait.associated_values {
                                resolver.visit_type(&associated_value.ty);
                            }
                            for method in &item_trait.methods {
                                resolver.visit_function(&method.function);
                            }
                        });
                    });
                });
            }
            ItemTreeNodeKind::Extend(extend) => {
                self.with_generics(&extend.generics, |resolver| {
                    let associated_types = extend
                        .associated_types
                        .iter()
                        .map(|associated_type| associated_type.name.clone())
                        .collect::<Vec<_>>();
                    resolver.with_self_type(extend.target.span, |resolver| {
                        resolver.visit_type(&extend.target);
                        if let Some(trait_ref) = &extend.trait_ref {
                            resolver.visit_type(trait_ref);
                        }
                        resolver.visit_where_clause(&extend.where_clause);
                        for associated_type in &extend.associated_types {
                            resolver.visit_type(&associated_type.ty);
                        }
                        for associated_value in &extend.associated_values {
                            if let Some(ty) = &associated_value.binding.ty {
                                resolver.visit_type(ty);
                            }
                            if let Some(value) = &associated_value.binding.value {
                                resolver.visit_expr(value);
                            }
                        }
                        resolver.with_associated_types(associated_types, |resolver| {
                            for method in &extend.methods {
                                resolver.visit_function(&method.function);
                            }
                        });
                    });
                });
            }
            ItemTreeNodeKind::TypeAlias(alias) => {
                self.with_generics(&alias.generics, |resolver| {
                    resolver.visit_where_clause(&alias.where_clause);
                    if let Some(ty) = &alias.ty {
                        resolver.visit_type(ty);
                    }
                });
            }
            ItemTreeNodeKind::Enum(item_enum) => {
                if let Some(backing_type) = &item_enum.backing_type {
                    self.visit_type(backing_type);
                }
                if self.mode == TypeResolveMode::All {
                    for variant in &item_enum.variants {
                        if let Some(value) = &variant.value {
                            self.visit_expr(value);
                        }
                    }
                }
            }
            ItemTreeNodeKind::Binding(binding) => {
                if let Some(ty) = &binding.ty {
                    self.visit_type(ty);
                }
                if self.mode == TypeResolveMode::All
                    && let Some(value) = &binding.value
                {
                    self.visit_expr(value);
                }
            }
            ItemTreeNodeKind::Function(function) => self.visit_function(function),
            ItemTreeNodeKind::Module(_) | ItemTreeNodeKind::Using(_) => {}
        }
    }

    fn visit_where_clause(&mut self, clause: &nia_ast::WhereClause) {
        for predicate in &clause.predicates {
            self.visit_type(&predicate.ty);
            for bound in &predicate.bounds {
                self.visit_type(bound);
            }
        }
    }

    fn visit_function(&mut self, function: &FunctionItem) {
        self.with_generics(&function.generics, |resolver| {
            resolver.visit_where_clause(&function.where_clause);
            for param in &function.params {
                if let Some(ty) = &param.ty {
                    resolver.visit_type(ty);
                }
            }
            if let Some(return_type) = &function.return_type {
                resolver.visit_type(return_type);
            }
            if resolver.mode == TypeResolveMode::All
                && let Some(body) = &function.body
            {
                resolver.visit_block(body);
            }
        });
    }
}

impl<'a> TypeResolver<'a> {
    fn resolve_type_path(&mut self, ty: &TypeRef, segments: &[TypePathSegment]) {
        let Some(first) = segments.first() else {
            return;
        };
        if segments.len() > 1 {
            let resolution = self.resolve_qualified_type_path(ty.span, &ty.node_key, segments);
            self.node_type_names
                .insert(ty.node_key.site().clone(), resolution);
            self.visit_type_path_args(segments);
            return;
        }
        let resolution = self.resolve_type_name(first, ty.span, &ty.node_key);
        self.node_type_names
            .insert(ty.node_key.site().clone(), resolution);
        self.visit_type_path_args(segments);
    }

    fn visit_type_path_args(&mut self, segments: &[TypePathSegment]) {
        for segment in segments {
            for arg in &segment.args {
                match arg {
                    TypeArg::Type(ty) => {
                        self.visit_type(ty);
                    }
                    TypeArg::AssocBinding { key, ty, .. } => {
                        self.visit_assoc_binding_key(key);
                        self.visit_type(ty);
                    }
                    TypeArg::Const(expr) => self.visit_expr(expr),
                    TypeArg::TypeOrConst { ty, .. } => self.resolve_type_candidate(ty),
                }
            }
        }
    }

    fn resolve_type_candidate(&mut self, ty: &TypeRef) {
        match &ty.kind {
            TypeKind::Path { segments } => {
                let Some(resolution) = self.try_resolve_type_path(ty, segments) else {
                    return;
                };
                self.node_type_names
                    .insert(ty.node_key.site().clone(), resolution);
                self.visit_type_path_args(segments);
            }
            TypeKind::Projection { ty, trait_ref, .. } => {
                self.resolve_type_candidate(ty);
                self.resolve_type_candidate(trait_ref);
            }
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePointer { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::SlicePointee { elem }
            | TypeKind::Optional { elem }
            | TypeKind::ErrorUnion { error: elem, .. } => self.resolve_type_candidate(elem),
            TypeKind::Array { elem, .. } => self.resolve_type_candidate(elem),
            TypeKind::Range { start, end, .. } => {
                if let Some(start) = start {
                    self.resolve_type_candidate(start);
                }
                if let Some(end) = end {
                    self.resolve_type_candidate(end);
                }
            }
            TypeKind::FunctionPointer {
                params,
                return_type,
                ..
            } => {
                for param in params {
                    self.resolve_type_candidate(param);
                }
                if let Some(return_type) = return_type {
                    self.resolve_type_candidate(return_type);
                }
            }
            TypeKind::Error
            | TypeKind::SelfType
            | TypeKind::Infer
            | TypeKind::Void
            | TypeKind::Never => {}
        }
    }

    fn try_resolve_type_path(
        &mut self,
        ty: &TypeRef,
        segments: &[TypePathSegment],
    ) -> Option<TypeNameResolution> {
        let first = segments.first()?;
        if segments.len() > 1 {
            let resolution = self.try_resolve_qualified_type_path(ty, segments)?;
            return Some(resolution);
        }
        let resolution = self.try_resolve_type_name(first, ty)?;
        Some(resolution)
    }

    fn try_resolve_qualified_type_path(
        &mut self,
        ty: &TypeRef,
        segments: &[TypePathSegment],
    ) -> Option<TypeNameResolution> {
        let (last, prefix) = segments.split_last()?;
        let namespace = self.try_resolve_namespace_path(prefix)?;
        match namespace {
            ResolvedNamespace::Module(module_id) => {
                self.try_resolve_module_type(ty, module_id, last)
            }
            ResolvedNamespace::Type(_) => None,
        }
    }

    fn try_resolve_namespace_path(
        &self,
        segments: &[TypePathSegment],
    ) -> Option<ResolvedNamespace> {
        let first = segments.first()?;
        let mut namespace = self.try_resolve_root_namespace(first)?;
        for segment in &segments[1..] {
            namespace = self.try_resolve_child_namespace(namespace, segment)?;
        }
        Some(namespace)
    }

    fn try_resolve_root_namespace(&self, segment: &TypePathSegment) -> Option<ResolvedNamespace> {
        if let Some(module_id) = self.root_module_for_segment(segment) {
            return Some(ResolvedNamespace::Module(module_id));
        }
        if let Some(scope) = self.using_scope
            && let Some(module_id) = scope.lookup_module(type_segment_name(segment)?)
        {
            return Some(ResolvedNamespace::Module(module_id));
        }
        if let Some(def_id) = self
            .defs
            .module_scope
            .types
            .get(type_segment_name(segment)?)
        {
            return Some(ResolvedNamespace::Type(GlobalDefId {
                module_id: self.defs.module_id,
                def_id,
            }));
        }
        if let Some(scope) = self.using_scope
            && let Some(entry) = scope.lookup_type(type_segment_name(segment)?)
        {
            return Some(ResolvedNamespace::Type(GlobalDefId {
                module_id: entry.target_module,
                def_id: entry.target_def_id,
            }));
        }
        None
    }

    fn try_resolve_child_namespace(
        &self,
        namespace: ResolvedNamespace,
        segment: &TypePathSegment,
    ) -> Option<ResolvedNamespace> {
        match namespace {
            ResolvedNamespace::Module(module_id) => {
                if let Some(surfaces) = self.public_surfaces
                    && let Some(surface) = surfaces.get(module_id)
                {
                    if let Some(child_module) = surface.lookup_module(type_segment_name(segment)?) {
                        return Some(ResolvedNamespace::Module(child_module));
                    }
                    if let Some(item) = surface.lookup_type(type_segment_name(segment)?) {
                        return Some(ResolvedNamespace::Type(GlobalDefId {
                            module_id: item.target_module,
                            def_id: item.target_def_id,
                        }));
                    }
                }
                if let Some((child_module, visibility)) =
                    self.child_module_declaration(module_id, type_segment_name(segment)?)
                    && self.module_declaration_visible(module_id, visibility)
                {
                    return Some(ResolvedNamespace::Module(child_module));
                }
                match self.direct_type_member(module_id, type_segment_name(segment)?) {
                    DirectMember::Visible(def_id) => {
                        Some(ResolvedNamespace::Type(GlobalDefId { module_id, def_id }))
                    }
                    DirectMember::Private | DirectMember::Missing | DirectMember::Unloaded => None,
                }
            }
            ResolvedNamespace::Type(_) => None,
        }
    }

    fn try_resolve_module_type(
        &mut self,
        ty: &TypeRef,
        module_id: ModuleId,
        segment: &TypePathSegment,
    ) -> Option<TypeNameResolution> {
        if let Some(surfaces) = self.public_surfaces
            && let Some(surface) = surfaces.get(module_id)
            && let Some(item) = surface.lookup_type(type_segment_name(segment)?)
        {
            let global = GlobalDefId {
                module_id: item.target_module,
                def_id: item.target_def_id,
            };
            if let Some(trait_id) =
                self.canonical_builtin_trait(global, type_segment_name(segment)?)
            {
                return Some(TypeNameResolution::BuiltinTrait(trait_id));
            }
            self.node_qualified_type_names
                .insert(ty.node_key.site().clone(), global);
            return Some(TypeNameResolution::External(global));
        }
        let DirectMember::Visible(def_id) =
            self.direct_type_member(module_id, type_segment_name(segment)?)
        else {
            return None;
        };
        let target_defs = self.defs_for_module(module_id)?;
        let target_defs = target_defs.as_ref();
        let def = target_defs.defs.get(def_id)?;
        if !matches!(
            def.kind,
            DefKind::Struct | DefKind::Union | DefKind::Trait | DefKind::Enum | DefKind::TypeAlias
        ) {
            return None;
        }
        if let Some(trait_id) =
            self.canonical_builtin_trait(GlobalDefId { module_id, def_id }, &def.name)
        {
            return Some(TypeNameResolution::BuiltinTrait(trait_id));
        }
        self.node_qualified_type_names.insert(
            ty.node_key.site().clone(),
            GlobalDefId { module_id, def_id },
        );
        if module_id == self.defs.module_id {
            Some(TypeNameResolution::Def(def_id))
        } else {
            Some(TypeNameResolution::External(GlobalDefId {
                module_id,
                def_id,
            }))
        }
    }

    fn try_resolve_type_name(
        &mut self,
        segment: &TypePathSegment,
        ty: &TypeRef,
    ) -> Option<TypeNameResolution> {
        if self.is_generic_param(type_segment_name(segment)?) {
            return Some(TypeNameResolution::GenericParam);
        }
        if self.is_associated_type(type_segment_name(segment)?) {
            return Some(TypeNameResolution::AssociatedType);
        }
        if let Some(primitive) =
            self.primitive_type_spelling_for_symbol(type_segment_name(segment)?)
        {
            return Some(TypeNameResolution::Primitive(primitive));
        }
        if let Some(def_id) = self
            .defs
            .module_scope
            .types
            .get(type_segment_name(segment)?)
        {
            let def = self.defs.defs.get(def_id)?;
            if matches!(
                def.kind,
                DefKind::Struct
                    | DefKind::Union
                    | DefKind::Trait
                    | DefKind::Enum
                    | DefKind::TypeAlias
            ) {
                if let Some(trait_id) = self.canonical_builtin_trait(
                    GlobalDefId {
                        module_id: self.defs.module_id,
                        def_id,
                    },
                    &def.name,
                ) {
                    return Some(TypeNameResolution::BuiltinTrait(trait_id));
                }
                return Some(TypeNameResolution::Def(def_id));
            }
        }
        if let Some(scope) = self.using_scope
            && let Some(entry) = scope.lookup_type(type_segment_name(segment)?)
            && entry.namespace == PublicNamespace::Type
        {
            let global = GlobalDefId {
                module_id: entry.target_module,
                def_id: entry.target_def_id,
            };
            if let Some(trait_id) =
                self.canonical_builtin_trait(global, type_segment_name(segment)?)
            {
                return Some(TypeNameResolution::BuiltinTrait(trait_id));
            }
            self.node_qualified_type_names
                .insert(ty.node_key.site().clone(), global);
            return Some(TypeNameResolution::External(global));
        }
        builtin_trait_for_symbol(type_segment_name(segment)?).map(TypeNameResolution::BuiltinTrait)
    }

    fn visit_assoc_binding_key(&mut self, key: &AssocBindingKey) {
        let AssocBindingKey::Projection(projection) = key else {
            return;
        };
        if let TypeKind::Projection { trait_ref, .. } = &projection.kind {
            self.visit_type(trait_ref);
        }
    }

    fn resolve_qualified_type_path(
        &mut self,
        span: Span,
        node_key: &VersionedNodeKey,
        segments: &[TypePathSegment],
    ) -> TypeNameResolution {
        let Some((last, prefix)) = segments.split_last() else {
            return TypeNameResolution::Error;
        };
        let Some(namespace) = self.resolve_namespace_path(span, prefix) else {
            return TypeNameResolution::Error;
        };
        match namespace {
            ResolvedNamespace::Module(module_id) => {
                let path_text = self.type_path_text(segments);
                self.resolve_module_type(span, node_key, module_id, last, &path_text)
            }
            ResolvedNamespace::Type(_) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::NAME_RESOLUTION,
                    span,
                    "type namespaces do not contain nested types",
                ));
                TypeNameResolution::Error
            }
        }
    }

    fn resolve_namespace_path(
        &mut self,
        path_span: Span,
        segments: &[TypePathSegment],
    ) -> Option<ResolvedNamespace> {
        let first = segments.first()?;
        let mut namespace = self.resolve_root_namespace(path_span, first)?;
        for segment in &segments[1..] {
            namespace = self.resolve_child_namespace(path_span, namespace, segment)?;
        }
        Some(namespace)
    }

    fn resolve_root_namespace(
        &mut self,
        path_span: Span,
        segment: &TypePathSegment,
    ) -> Option<ResolvedNamespace> {
        if let Some(module_id) = self.root_module_for_segment(segment) {
            return Some(ResolvedNamespace::Module(module_id));
        }
        if let Some(scope) = self.using_scope
            && let Some(module_id) = scope.lookup_module(type_segment_name(segment)?)
        {
            return Some(ResolvedNamespace::Module(module_id));
        }
        if let Some(def_id) = self
            .defs
            .module_scope
            .types
            .get(type_segment_name(segment)?)
        {
            return Some(ResolvedNamespace::Type(GlobalDefId {
                module_id: self.defs.module_id,
                def_id,
            }));
        }
        if let Some(scope) = self.using_scope
            && let Some(entry) = scope.lookup_type(type_segment_name(segment)?)
        {
            return Some(ResolvedNamespace::Type(GlobalDefId {
                module_id: entry.target_module,
                def_id: entry.target_def_id,
            }));
        }
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::NAME_RESOLUTION,
            path_span,
            format!(
                "unknown namespace `{}`",
                self.symbol_name(*type_segment_name(segment)?)
            ),
        ));
        None
    }

    fn root_module_for_segment(&self, segment: &TypePathSegment) -> Option<ModuleId> {
        let graph = self.graph()?;
        graph.root_module_for_segment(
            self.defs.module_id,
            module_root_segment_from_path_segment(segment.kind),
        )
    }

    fn resolve_child_namespace(
        &mut self,
        path_span: Span,
        namespace: ResolvedNamespace,
        segment: &TypePathSegment,
    ) -> Option<ResolvedNamespace> {
        match namespace {
            ResolvedNamespace::Module(module_id) => {
                if let Some(surfaces) = self.public_surfaces
                    && let Some(surface) = surfaces.get(module_id)
                {
                    if let Some(child_module) = surface.lookup_module(type_segment_name(segment)?) {
                        return Some(ResolvedNamespace::Module(child_module));
                    }
                    if let Some(item) = surface.lookup_type(type_segment_name(segment)?) {
                        return Some(ResolvedNamespace::Type(GlobalDefId {
                            module_id: item.target_module,
                            def_id: item.target_def_id,
                        }));
                    }
                }
                if let Some((child_module, visibility)) =
                    self.child_module_declaration(module_id, type_segment_name(segment)?)
                {
                    if self.module_declaration_visible(module_id, visibility) {
                        return Some(ResolvedNamespace::Module(child_module));
                    }
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::NAME_RESOLUTION,
                        path_span,
                        format!(
                            "module namespace `{}` is private",
                            self.symbol_name(*type_segment_name(segment)?)
                        ),
                    ));
                    return None;
                }
                match self.direct_type_member(module_id, type_segment_name(segment)?) {
                    DirectMember::Visible(def_id) => {
                        Some(ResolvedNamespace::Type(GlobalDefId { module_id, def_id }))
                    }
                    DirectMember::Private => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::NAME_RESOLUTION,
                            path_span,
                            format!(
                                "type `{}` is private",
                                self.symbol_name(*type_segment_name(segment)?)
                            ),
                        ));
                        None
                    }
                    DirectMember::Missing => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::NAME_RESOLUTION,
                            path_span,
                            format!(
                                "unknown namespace `{}`",
                                self.symbol_name(*type_segment_name(segment)?)
                            ),
                        ));
                        None
                    }
                    DirectMember::Unloaded => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::NAME_RESOLUTION,
                            path_span,
                            "module namespace refers to an unloaded module",
                        ));
                        None
                    }
                }
            }
            ResolvedNamespace::Type(_) => None,
        }
    }

    fn resolve_module_type(
        &mut self,
        span: Span,
        node_key: &VersionedNodeKey,
        module_id: ModuleId,
        segment: &TypePathSegment,
        path_text: &str,
    ) -> TypeNameResolution {
        let Some(name) = type_segment_name(segment).copied() else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                span,
                format!(
                    "expected type name, found `{}`",
                    self.type_segment_display(segment)
                ),
            ));
            return TypeNameResolution::Error;
        };
        if let Some(surfaces) = self.public_surfaces
            && let Some(surface) = surfaces.get(module_id)
            && let Some(item) = surface.lookup_type(&name)
        {
            let global = GlobalDefId {
                module_id: item.target_module,
                def_id: item.target_def_id,
            };
            if let Some(trait_id) = self.canonical_builtin_trait(global, &name) {
                return TypeNameResolution::BuiltinTrait(trait_id);
            }
            self.node_qualified_type_names
                .insert(node_key.site().clone(), global);
            return TypeNameResolution::External(global);
        }
        let def_id = match self.direct_type_member(module_id, &name) {
            DirectMember::Visible(def_id) => def_id,
            DirectMember::Private => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::NAME_RESOLUTION,
                    span,
                    format!("type `{path_text}` is private"),
                ));
                return TypeNameResolution::Error;
            }
            DirectMember::Missing => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::NAME_RESOLUTION,
                    span,
                    format!("unknown type `{}`", self.symbol_name(name)),
                ));
                return TypeNameResolution::Error;
            }
            DirectMember::Unloaded => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::NAME_RESOLUTION,
                    span,
                    "module namespace refers to an unloaded module",
                ));
                return TypeNameResolution::Error;
            }
        };
        let Some(target_defs) = self.defs_for_module(module_id) else {
            return TypeNameResolution::Error;
        };
        let target_defs = target_defs.as_ref();
        let Some(def) = target_defs.defs.get(def_id) else {
            return TypeNameResolution::Error;
        };
        if !matches!(
            def.kind,
            DefKind::Struct | DefKind::Union | DefKind::Trait | DefKind::Enum | DefKind::TypeAlias
        ) {
            return TypeNameResolution::Error;
        }
        if let Some(trait_id) =
            self.canonical_builtin_trait(GlobalDefId { module_id, def_id }, &def.name)
        {
            return TypeNameResolution::BuiltinTrait(trait_id);
        }
        self.node_qualified_type_names
            .insert(node_key.site().clone(), GlobalDefId { module_id, def_id });
        if module_id == self.defs.module_id {
            TypeNameResolution::Def(def_id)
        } else {
            TypeNameResolution::External(GlobalDefId { module_id, def_id })
        }
    }

    fn resolve_type_name(
        &mut self,
        segment: &TypePathSegment,
        span: Span,
        node_key: &VersionedNodeKey,
    ) -> TypeNameResolution {
        let Some(name) = type_segment_name(segment).copied() else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                span,
                format!(
                    "expected type name, found `{}`",
                    self.type_segment_display(segment)
                ),
            ));
            return TypeNameResolution::Error;
        };
        if self.is_generic_param(&name) {
            return TypeNameResolution::GenericParam;
        }
        if self.is_associated_type(&name) {
            return TypeNameResolution::AssociatedType;
        }
        if let Some(primitive) = self.primitive_type_spelling_for_symbol(&name) {
            return TypeNameResolution::Primitive(primitive);
        }
        if let Some(def_id) = self.defs.module_scope.types.get(&name) {
            let Some(def) = self.defs.defs.get(def_id) else {
                return TypeNameResolution::Error;
            };
            if matches!(
                def.kind,
                DefKind::Struct
                    | DefKind::Union
                    | DefKind::Trait
                    | DefKind::Enum
                    | DefKind::TypeAlias
            ) {
                if let Some(trait_id) = self.canonical_builtin_trait(
                    GlobalDefId {
                        module_id: self.defs.module_id,
                        def_id,
                    },
                    &def.name,
                ) {
                    return TypeNameResolution::BuiltinTrait(trait_id);
                }
                return TypeNameResolution::Def(def_id);
            }
        }
        if let Some(scope) = self.using_scope
            && let Some(entry) = scope.lookup_type(&name)
            && entry.namespace == PublicNamespace::Type
        {
            let global = GlobalDefId {
                module_id: entry.target_module,
                def_id: entry.target_def_id,
            };
            if let Some(trait_id) = self.canonical_builtin_trait(global, &name) {
                return TypeNameResolution::BuiltinTrait(trait_id);
            }
            self.node_qualified_type_names
                .insert(node_key.site().clone(), global);
            return TypeNameResolution::External(global);
        }
        if self
            .using_scope
            .is_some_and(|scope| scope.has_unresolved_name(&name))
        {
            return TypeNameResolution::Error;
        }
        if let Some(trait_id) = builtin_trait_for_symbol(&name) {
            return TypeNameResolution::BuiltinTrait(trait_id);
        }
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::NAME_RESOLUTION,
            span,
            format!("unknown type `{}`", self.symbol_name(name)),
        ));
        TypeNameResolution::Error
    }

    fn with_generics(&mut self, generics: &[GenericParam], f: impl FnOnce(&mut Self)) {
        for generic in generics {
            if let GenericParamKind::Comptime { ty } = &generic.kind {
                self.visit_type(ty);
            }
        }
        self.generic_stack.push(generics.to_vec());
        f(self);
        self.generic_stack.pop();
    }

    fn with_self_type(&mut self, span: Span, f: impl FnOnce(&mut Self)) {
        self.self_type_stack.push(span);
        f(self);
        self.self_type_stack.pop();
    }

    fn with_associated_types(
        &mut self,
        associated_types: Vec<SymbolId>,
        f: impl FnOnce(&mut Self),
    ) {
        self.associated_type_stack.push(associated_types);
        f(self);
        self.associated_type_stack.pop();
    }

    fn is_generic_param(&self, name: &SymbolId) -> bool {
        self.generic_stack
            .iter()
            .rev()
            .any(|generics| generics.iter().any(|generic| &generic.name == name))
    }

    fn is_comptime_generic_param(&self, name: &SymbolId) -> bool {
        self.generic_stack.iter().rev().any(|generics| {
            generics.iter().any(|generic| {
                &generic.name == name && matches!(generic.kind, GenericParamKind::Comptime { .. })
            })
        })
    }

    fn is_associated_type(&self, name: &SymbolId) -> bool {
        self.associated_type_stack
            .iter()
            .rev()
            .any(|associated_types| associated_types.iter().any(|associated| associated == name))
    }

    fn defs_for_module(&self, module_id: ModuleId) -> Option<ModuleDefs<'_>> {
        if module_id == self.defs.module_id {
            Some(ModuleDefs::Borrowed(self.defs))
        } else {
            Some(ModuleDefs::Owned((self.program_defs.defs?)(module_id)?))
        }
    }

    fn canonical_builtin_trait(
        &self,
        global: GlobalDefId,
        expected_name: &SymbolId,
    ) -> Option<BuiltinTrait> {
        let graph = self.graph()?;
        let module = graph.get(global.module_id)?;
        if module.module_path.package != known::std()
            || module
                .module_path
                .segments
                .first()
                .is_none_or(|segment| *segment != known::builtin())
        {
            return None;
        }
        let target_defs = self.defs_for_module(global.module_id)?;
        let def = target_defs.as_ref().defs.get(global.def_id)?;
        if def.kind != DefKind::Trait || &def.name != expected_name {
            return None;
        }
        builtin_trait_for_symbol(&def.name)
    }
}

fn module_root_segment_from_path_segment(kind: PathSegmentKind) -> ModuleRootSegment {
    match kind {
        PathSegmentKind::SelfValue => ModuleRootSegment::Current,
        PathSegmentKind::Super => ModuleRootSegment::Parent,
        PathSegmentKind::Package => ModuleRootSegment::PackageRelative,
        PathSegmentKind::Name(name) => ModuleRootSegment::Named(name),
    }
}

fn type_segment_name(segment: &TypePathSegment) -> Option<&SymbolId> {
    match &segment.kind {
        PathSegmentKind::Name(name) => Some(name),
        PathSegmentKind::Package | PathSegmentKind::Super | PathSegmentKind::SelfValue => None,
    }
}

fn primitive_type_spelling_for_known_symbol(name: &SymbolId) -> Option<PrimitiveTypeSpelling> {
    let scalar = match *name {
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
        known::VOID => PrimitiveTy::Void,
        known::NEVER => PrimitiveTy::Never,
        _ => return None,
    };
    Some(PrimitiveTypeSpelling::Scalar(scalar))
}

fn builtin_trait_for_symbol(name: &SymbolId) -> Option<BuiltinTrait> {
    Some(match *name {
        known::ADD_TRAIT => BuiltinTrait::Add,
        known::SUB_TRAIT => BuiltinTrait::Sub,
        known::MUL_TRAIT => BuiltinTrait::Mul,
        known::DIV_TRAIT => BuiltinTrait::Div,
        known::REM_TRAIT => BuiltinTrait::Rem,
        known::NEG_TRAIT => BuiltinTrait::Neg,
        known::NOT_TRAIT => BuiltinTrait::Not,
        known::BIT_NOT_TRAIT => BuiltinTrait::BitNot,
        known::BIT_AND_TRAIT => BuiltinTrait::BitAnd,
        known::BIT_OR_TRAIT => BuiltinTrait::BitOr,
        known::BIT_XOR_TRAIT => BuiltinTrait::BitXor,
        known::SHL_TRAIT => BuiltinTrait::Shl,
        known::SHR_TRAIT => BuiltinTrait::Shr,
        known::EQ_TRAIT => BuiltinTrait::Eq,
        known::ORD_TRAIT => BuiltinTrait::Ord,
        known::SIZED_TRAIT => BuiltinTrait::Sized,
        known::UNSIZED_TRAIT => BuiltinTrait::Unsized,
        known::DEREF_TRAIT => BuiltinTrait::Deref,
        known::DEREF_MUT_TRAIT => BuiltinTrait::DerefMut,
        known::INDEX_TRAIT => BuiltinTrait::Index,
        known::INDEX_MUT_TRAIT => BuiltinTrait::IndexMut,
        known::SLICE_TRAIT => BuiltinTrait::Slice,
        known::SLICE_MUT_TRAIT => BuiltinTrait::SliceMut,
        known::PTR_TRAIT => BuiltinTrait::Ptr,
        known::PTR_MUT_TRAIT => BuiltinTrait::PtrMut,
        known::LEN_TRAIT => BuiltinTrait::Len,
        known::START_TRAIT => BuiltinTrait::Start,
        known::END_TRAIT => BuiltinTrait::End,
        known::CHAR_TRAIT => BuiltinTrait::Char,
        known::ITERABLE_TRAIT => BuiltinTrait::Iterable,
        known::ITERATOR_TRAIT => BuiltinTrait::Iterator,
        known::SIMD_TRAIT => BuiltinTrait::Simd,
        known::SIMD_MASK_TRAIT => BuiltinTrait::SimdMask,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{ModuleId, collect_module_defs, collect_module_defs_from_active_item_tree};
    use nia_item_tree::ModuleItemTree;
    use nia_parser::parse_module_with_symbols;
    use nia_symbol_table::SymbolTable;

    fn resolve_source(source: &str) -> TypeResolution {
        let symbols = SymbolTable::new();
        let (module, errors) = parse_module_with_symbols(source, symbols.clone());
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
        resolve_module_types_with_symbols(&module, &defs, &symbols)
    }

    #[test]
    fn resolves_primitive_nominal_and_generic_types() {
        let resolved = resolve_source(
            r#"
struct Box[T] {
    value: T,
}

type Byte = u8;

fn make(value: i32) Box[i32] {
    let mut tmp: Byte = 1;
    { value: value }
}
"#,
        );
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        assert!(
            resolved
                .node_type_names
                .values()
                .any(|resolution| matches!(resolution, TypeNameResolution::GenericParam))
        );
        assert!(
            resolved
                .node_type_names
                .values()
                .any(|resolution| matches!(resolution, TypeNameResolution::Primitive(_)))
        );
        assert!(
            resolved
                .node_type_names
                .values()
                .any(|resolution| matches!(resolution, TypeNameResolution::Def(_)))
        );
    }

    #[test]
    fn resolves_trait_associated_type_shorthand_in_trait_scope() {
        let resolved = resolve_source(
            r#"
trait Writer {
    type Error;

    fn write(& self) Error!void;
}
"#,
        );
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        assert!(
            resolved
                .node_type_names
                .values()
                .any(|resolution| { matches!(resolution, TypeNameResolution::AssociatedType) })
        );
    }

    #[test]
    fn resolves_trait_impl_associated_type_shorthand_before_builtin_error() {
        let resolved = resolve_source(
            r#"
trait Reader {
    type Error;

    fn end_of_stream(&self) Error;
}

struct Buffer {}

extend Buffer : Reader {
    type Error = i32;

    fn end_of_stream(&self) Error {
        1
    }
}
"#,
        );
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        let associated_type_count = resolved
            .node_type_names
            .values()
            .filter(|resolution| matches!(resolution, TypeNameResolution::AssociatedType))
            .count();
        assert_eq!(associated_type_count, 2);
    }

    #[test]
    fn local_types_shadow_builtin_trait_fallback_names() {
        let resolved = resolve_source(
            r#"
type Ptr[T] = &T;

fn id(value: Ptr[u8]) Ptr[u8] {
    value
}
"#,
        );
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        assert!(
            resolved
                .node_type_names
                .values()
                .any(|resolution| matches!(resolution, TypeNameResolution::Def(_)))
        );
        assert!(
            !resolved.node_type_names.values().any(|resolution| matches!(
                resolution,
                TypeNameResolution::BuiltinTrait(BuiltinTrait::Ptr)
            ))
        );
    }

    #[test]
    fn reports_unknown_types_without_resolving_values() {
        let resolved = resolve_source(
            r#"
fn main() Missing {
    let mut value = MissingValue;
    0
}
"#,
        );
        assert_eq!(resolved.diagnostics.len(), 1);
        assert!(
            resolved.diagnostics[0]
                .summary
                .contains("unknown type `Missing`")
        );
    }

    #[test]
    fn reports_qualified_namespace_errors_on_type_path_span() {
        let resolved = resolve_source(
            r#"
fn main() Missing::Type {
    0
}
"#,
        );
        assert_eq!(resolved.diagnostics.len(), 1);
        assert!(
            resolved.diagnostics[0]
                .summary
                .contains("unknown namespace `Missing`")
        );
        assert_ne!(
            resolved.diagnostics[0].primary_span(),
            Some(Span::default())
        );
    }

    #[test]
    fn resolves_types_from_active_item_tree_only() {
        let symbols = SymbolTable::new();
        let (module, errors) = parse_module_with_symbols(
            r#"
@[if false]
fn skipped(value: MissingType) void {}
@[if true]
fn selected(value: i32) void {}
"#,
            symbols.clone(),
        );
        assert!(errors.is_empty(), "{errors:?}");
        let tree = ModuleItemTree::from_module(&module);
        let active = tree.active_items(&mut BoolResolver(false)).unwrap();
        let defs = collect_module_defs_from_active_item_tree(ModuleId(0), &active);
        assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
        let resolved = resolve_module_types_from_active_item_tree_with_symbols(
            &active,
            &defs,
            ProgramDefsContext::empty(),
            &nia_defs::PublicSurfaces::default(),
            &nia_defs::ModuleUsingScope::default(),
            &symbols,
        );
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        assert!(
            resolved
                .node_type_names
                .values()
                .any(|resolution| matches!(resolution, TypeNameResolution::Primitive(_)))
        );
    }

    struct BoolResolver(bool);

    impl nia_item_tree::ConditionResolver for BoolResolver {
        fn resolve_condition(
            &mut self,
            cond: &nia_ast::ConditionExpr,
        ) -> Result<bool, nia_item_tree::ItemTreeError> {
            match &cond.kind {
                nia_ast::ConditionExprKind::Bool(value) => Ok(*value),
                _ => Ok(self.0),
            }
        }
    }
}
