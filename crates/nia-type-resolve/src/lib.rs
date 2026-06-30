// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{
    ArrayLen, AssocBindingKey, FunctionItem, Item, ItemKind, Module, TypeArg, TypeKind,
    TypePathSegment, TypeRef,
};
use nia_ast_walk::{Visitor, walk_function, walk_item};
use nia_defs::{DefCollection, DefKind, ModuleUsingScope, PublicNamespace, PublicSurfaces};
use nia_diagnostic::{Diagnostic, codes};
pub use nia_ids::DefId;
use nia_ids::{GlobalDefId, ModuleId, Visibility};
use nia_imports::{
    ENTRY_MODULE_MAP_NAME, ModuleGraph, PACKAGE_MODULE_MAP_NAME, STD_MODULE_MAP_NAME,
    module_declaration_visibility_allows, visibility_allows,
};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNode, ItemTreeNodeKind, ModuleItemTree};
use nia_node_id::{NodeSite, VersionedNodeKey};
use nia_span::Span;
use nia_ty::{BuiltinTrait, PrimitiveTypeSpelling};

#[derive(Debug, Clone, PartialEq)]
pub struct TypeResolution {
    pub node_type_names: HashMap<NodeSite, TypeNameResolution>,
    pub node_qualified_type_names: HashMap<NodeSite, GlobalDefId>,
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
) -> TypeResolution {
    resolve_module_types_from_items_with_mode(
        &item_tree.items,
        defs,
        graph,
        program_defs,
        public_surfaces,
        using_scope,
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
) -> TypeResolution {
    resolve_module_types_from_items_with_mode(
        items,
        defs,
        graph,
        program_defs,
        public_surfaces,
        using_scope,
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
    mode: TypeResolveMode,
) -> TypeResolution {
    let mut resolver = TypeResolver {
        defs,
        graph,
        program_defs,
        public_surfaces,
        using_scope,
        node_type_names: HashMap::new(),
        node_qualified_type_names: HashMap::new(),
        diagnostics: Vec::new(),
        generic_stack: Vec::new(),
        self_type_stack: Vec::new(),
        associated_type_stack: Vec::new(),
        suppress_unknown_type_errors: false,
        mode,
    };
    for item in items {
        resolver.visit_item_tree_node(item);
    }
    TypeResolution {
        node_type_names: resolver.node_type_names,
        node_qualified_type_names: resolver.node_qualified_type_names,
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
    node_type_names: HashMap<NodeSite, TypeNameResolution>,
    node_qualified_type_names: HashMap<NodeSite, GlobalDefId>,
    diagnostics: Vec<Diagnostic>,
    generic_stack: Vec<Vec<String>>,
    self_type_stack: Vec<Span>,
    associated_type_stack: Vec<Vec<String>>,
    suppress_unknown_type_errors: bool,
    mode: TypeResolveMode,
}

impl TypeResolver<'_> {
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
        name: &str,
    ) -> Option<(ModuleId, Visibility)> {
        let graph = self.graph()?;
        let parent = graph.get(parent_module)?;
        let target = parent.children.get(name).copied()?;
        let declaration = parent
            .declarations
            .iter()
            .find(|declaration| declaration.name == name && declaration.target == target)?;
        Some((target, declaration.visibility))
    }

    fn direct_type_member(&self, module_id: ModuleId, name: &str) -> DirectMember<DefId> {
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
            nia_ast::ExprKind::BracketSuffix { callee, args } => {
                self.visit_expr(callee);
                for arg in args {
                    if let Some(expr) = &arg.expr {
                        nia_ast_walk::walk_expr(self, expr);
                    }
                    if let Some(ty) = &arg.ty {
                        // A bracket suffix in expression position is ambiguous
                        // until local/value resolution decides whether it is an
                        // index or generic call. Do resolve nested type args so
                        // real names get recorded, but suppress "unknown type"
                        // here to avoid false errors for index expressions like
                        // `xs[i32]` where `i32` is a local.
                        self.with_suppressed_unknown_type_errors(|resolver| {
                            resolver.visit_type(ty);
                        });
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
                    resolver.visit_type(&alias.ty);
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
                    TypeArg::Type(ty) => self.visit_type(ty),
                    TypeArg::AssocBinding { key, ty, .. } => {
                        self.visit_assoc_binding_key(key);
                        self.visit_type(ty);
                    }
                    TypeArg::Const(_) => {}
                }
            }
        }
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
                let path_text = type_path_text(segments);
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
        if let Some(module_id) = self.root_module_for_segment(&segment.name) {
            return Some(ResolvedNamespace::Module(module_id));
        }
        if let Some(scope) = self.using_scope
            && let Some(module_id) = scope.lookup_module(&segment.name)
        {
            return Some(ResolvedNamespace::Module(module_id));
        }
        if let Some(def_id) = self.defs.module_scope.types.get(&segment.name) {
            return Some(ResolvedNamespace::Type(GlobalDefId {
                module_id: self.defs.module_id,
                def_id,
            }));
        }
        if let Some(scope) = self.using_scope
            && let Some(entry) = scope.lookup_type(&segment.name)
        {
            return Some(ResolvedNamespace::Type(GlobalDefId {
                module_id: entry.target_module,
                def_id: entry.target_def_id,
            }));
        }
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::NAME_RESOLUTION,
            path_span,
            format!("unknown namespace `{}`", segment.name),
        ));
        None
    }

    fn root_module_for_segment(&self, name: &str) -> Option<ModuleId> {
        let graph = self.graph()?;
        match name {
            "self" => Some(self.defs.module_id),
            "super" => graph.get(self.defs.module_id)?.parent,
            ENTRY_MODULE_MAP_NAME => Some(graph.entry()),
            PACKAGE_MODULE_MAP_NAME => graph.current_package_root(self.defs.module_id),
            package => graph
                .get(self.defs.module_id)
                .and_then(|node| node.children.get(package).copied())
                .or_else(|| graph.package_root(package)),
        }
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
                    if let Some(child_module) = surface.lookup_module(&segment.name) {
                        return Some(ResolvedNamespace::Module(child_module));
                    }
                    if let Some(item) = surface.lookup_type(&segment.name) {
                        return Some(ResolvedNamespace::Type(GlobalDefId {
                            module_id: item.target_module,
                            def_id: item.target_def_id,
                        }));
                    }
                }
                if let Some((child_module, visibility)) =
                    self.child_module_declaration(module_id, &segment.name)
                {
                    if self.module_declaration_visible(module_id, visibility) {
                        return Some(ResolvedNamespace::Module(child_module));
                    }
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::NAME_RESOLUTION,
                        path_span,
                        format!("module namespace `{}` is private", segment.name),
                    ));
                    return None;
                }
                match self.direct_type_member(module_id, &segment.name) {
                    DirectMember::Visible(def_id) => {
                        Some(ResolvedNamespace::Type(GlobalDefId { module_id, def_id }))
                    }
                    DirectMember::Private => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::NAME_RESOLUTION,
                            path_span,
                            format!("type `{}` is private", segment.name),
                        ));
                        None
                    }
                    DirectMember::Missing => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::NAME_RESOLUTION,
                            path_span,
                            format!("unknown namespace `{}`", segment.name),
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
        if let Some(surfaces) = self.public_surfaces
            && let Some(surface) = surfaces.get(module_id)
            && let Some(item) = surface.lookup_type(&segment.name)
        {
            let global = GlobalDefId {
                module_id: item.target_module,
                def_id: item.target_def_id,
            };
            if let Some(trait_id) = self.canonical_builtin_trait(global, &segment.name) {
                return TypeNameResolution::BuiltinTrait(trait_id);
            }
            self.node_qualified_type_names
                .insert(node_key.site().clone(), global);
            return TypeNameResolution::External(global);
        }
        let def_id = match self.direct_type_member(module_id, &segment.name) {
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
                    format!("unknown type `{}`", segment.name),
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
        if self.is_generic_param(&segment.name) {
            return TypeNameResolution::GenericParam;
        }
        if self.is_associated_type(&segment.name) {
            return TypeNameResolution::AssociatedType;
        }
        if let Some(primitive) = PrimitiveTypeSpelling::from_name(&segment.name) {
            return TypeNameResolution::Primitive(primitive);
        }
        if let Some(def_id) = self.defs.module_scope.types.get(&segment.name) {
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
            && let Some(entry) = scope.lookup_type(&segment.name)
            && entry.namespace == PublicNamespace::Type
        {
            let global = GlobalDefId {
                module_id: entry.target_module,
                def_id: entry.target_def_id,
            };
            if let Some(trait_id) = self.canonical_builtin_trait(global, &segment.name) {
                return TypeNameResolution::BuiltinTrait(trait_id);
            }
            self.node_qualified_type_names
                .insert(node_key.site().clone(), global);
            return TypeNameResolution::External(global);
        }
        if self
            .using_scope
            .is_some_and(|scope| scope.has_unresolved_name(&segment.name))
        {
            return TypeNameResolution::Error;
        }
        if let Some(trait_id) = BuiltinTrait::from_name(&segment.name) {
            return TypeNameResolution::BuiltinTrait(trait_id);
        }
        if !self.suppress_unknown_type_errors {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                span,
                format!("unknown type `{}`", segment.name),
            ));
        }
        TypeNameResolution::Error
    }

    fn with_suppressed_unknown_type_errors(&mut self, f: impl FnOnce(&mut Self)) {
        let previous = self.suppress_unknown_type_errors;
        self.suppress_unknown_type_errors = true;
        f(self);
        self.suppress_unknown_type_errors = previous;
    }

    fn with_generics(&mut self, generics: &[String], f: impl FnOnce(&mut Self)) {
        self.generic_stack.push(generics.to_vec());
        f(self);
        self.generic_stack.pop();
    }

    fn with_self_type(&mut self, span: Span, f: impl FnOnce(&mut Self)) {
        self.self_type_stack.push(span);
        f(self);
        self.self_type_stack.pop();
    }

    fn with_associated_types(&mut self, associated_types: Vec<String>, f: impl FnOnce(&mut Self)) {
        self.associated_type_stack.push(associated_types);
        f(self);
        self.associated_type_stack.pop();
    }

    fn is_generic_param(&self, name: &str) -> bool {
        self.generic_stack
            .iter()
            .rev()
            .any(|generics| generics.iter().any(|generic| generic == name))
    }

    fn is_associated_type(&self, name: &str) -> bool {
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
        expected_name: &str,
    ) -> Option<BuiltinTrait> {
        let graph = self.graph()?;
        let module = graph.get(global.module_id)?;
        if module.module_path.package != STD_MODULE_MAP_NAME
            || module
                .module_path
                .segments
                .first()
                .is_none_or(|segment| segment != "builtin")
        {
            return None;
        }
        let target_defs = self.defs_for_module(global.module_id)?;
        let def = target_defs.as_ref().defs.get(global.def_id)?;
        if def.kind != DefKind::Trait || def.name != expected_name {
            return None;
        }
        BuiltinTrait::from_name(&def.name)
    }
}

fn type_path_text(segments: &[TypePathSegment]) -> String {
    segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>()
        .join("::")
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{ModuleId, collect_module_defs, collect_module_defs_from_active_item_tree};
    use nia_item_tree::ModuleItemTree;
    use nia_parser::parse_module;

    #[test]
    fn resolves_primitive_nominal_and_generic_types() {
        let (module, errors) = parse_module(
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
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
        let resolved = resolve_module_types(&module, &defs);
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
        let (module, errors) = parse_module(
            r#"
trait Writer {
    type Error;

    fn write(& self) Error!void;
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
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
        let (module, errors) = parse_module(
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
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
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
        let (module, errors) = parse_module(
            r#"
type Ptr[T] = &T;

fn id(value: Ptr[u8]) Ptr[u8] {
    value
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
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
        let (module, errors) = parse_module(
            r#"
fn main() Missing {
    let mut value = MissingValue;
    0
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        assert_eq!(resolved.diagnostics.len(), 1);
        assert!(
            resolved.diagnostics[0]
                .summary
                .contains("unknown type `Missing`")
        );
    }

    #[test]
    fn reports_qualified_namespace_errors_on_type_path_span() {
        let (module, errors) = parse_module(
            r#"
fn main() Missing::Type {
    0
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
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
        let (module, errors) = parse_module(
            r#"
@[if false]
fn skipped(value: MissingType) void {}
@[if true]
fn selected(value: i32) void {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let tree = ModuleItemTree::from_module(&module);
        let active = tree.active_items(&mut BoolResolver(false)).unwrap();
        let defs = collect_module_defs_from_active_item_tree(ModuleId(0), &active);
        assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
        let resolved = resolve_module_types_from_active_item_tree(
            &active,
            &defs,
            ProgramDefsContext::empty(),
            &nia_defs::PublicSurfaces::default(),
            &nia_defs::ModuleUsingScope::default(),
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
