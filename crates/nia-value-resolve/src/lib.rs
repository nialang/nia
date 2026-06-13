// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{Expr, ExprKind, Module, Visibility};
use nia_ast_walk::{Visitor, walk_expr, walk_where_clause};
use nia_defs::{
    DefCollection, DefKind, ModuleUsingScope, PublicNamespace, PublicSurfaces,
    VisibleExtensionMethods,
};
use nia_diagnostic::Diagnostic;
pub use nia_ids::DefId;
use nia_ids::{GlobalDefId, ModuleId};
use nia_imports::{
    ModuleGraph, PACKAGE_MODULE_MAP_NAME, ROOT_MODULE_MAP_NAME,
    module_declaration_visibility_allows, visibility_allows,
};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNode, ItemTreeNodeKind, ModuleItemTree};
use nia_node_id::NodeKey;
use nia_sema_ir::{BuiltinAssociatedValue, PrimitiveIntLimit, supports_primitive_int_limit};
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyInterner, TyKind};

#[derive(Debug, Clone, PartialEq)]
pub struct ValueResolution {
    pub node_names: HashMap<NodeKey, ValueNameResolution>,
    pub node_qualified_values: HashMap<NodeKey, GlobalDefId>,
    pub node_builtin_associated_values: HashMap<NodeKey, BuiltinAssociatedValue>,
    /// For spans whose value resolves to an enum variant (brought in via
    /// `using` or accessed as `mod::Enum::Variant`), the parent enum's
    /// GlobalDefId so consumers can type the bare ident as that enum.
    pub node_variant_enums: HashMap<NodeKey, GlobalDefId>,
    /// For `Qualified` spans like `mod::TypeName` appearing in expression
    /// position (e.g., as a type prefix in `mod::Enum::Variant` or
    /// `mod::Type::associated_fn(...)`), the resolved type's GlobalDefId.
    /// Populated by value-resolve so downstream phases can recognise these
    /// as type prefixes without re-resolving the module alias.
    pub node_qualified_type_prefixes: HashMap<NodeKey, GlobalDefId>,
    pub node_builtins: HashMap<NodeKey, BuiltinResolution>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueNameResolution {
    Def(DefId),
    External(GlobalDefId),
    Module,
    LocalDeferred,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinResolution {
    Builtin,
    ComptimeError,
    Trap,
    SizeOf,
    AlignOf,
    Asm,
    MemCopy,
    MemMove,
    MemSet,
    LoadUnaligned,
    Splat,
    Extract,
    Insert,
    Bitmask,
    Ctz,
    Clz,
    Popcount,
    AtomicLoad,
    AtomicStore,
    AtomicRmw,
    CmpxchgStrong,
    CmpxchgWeak,
    Fence,
    Reserved,
}

#[derive(Debug, Clone, Copy)]
pub struct ProgramDefsContext<'a> {
    pub defs: Option<&'a HashMap<ModuleId, DefCollection>>,
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
            extensions: None,
            extension_interner: None,
        },
    )
}

pub fn resolve_module_values_with_context(
    module: &Module,
    defs: &DefCollection,
    graph: &ModuleGraph,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &PublicSurfaces,
    using_scope: &ModuleUsingScope,
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
            extensions: None,
            extension_interner: None,
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
            extensions: None,
            extension_interner: None,
        },
    )
}

pub fn resolve_module_values_from_active_item_tree(
    item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &PublicSurfaces,
    using_scope: &ModuleUsingScope,
) -> ValueResolution {
    let empty_extensions = VisibleExtensionMethods::default();
    let empty_interner = TyInterner::default();
    resolve_module_values_from_active_item_tree_with_extensions(
        item_tree,
        defs,
        program_defs,
        public_surfaces,
        using_scope,
        &empty_extensions,
        &empty_interner,
    )
}

pub fn resolve_module_values_from_active_item_tree_with_extensions(
    item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &PublicSurfaces,
    using_scope: &ModuleUsingScope,
    extensions: &VisibleExtensionMethods,
    extension_interner: &TyInterner,
) -> ValueResolution {
    resolve_module_values_from_items(
        &item_tree.items,
        ValueResolveInputs {
            defs,
            graph: program_defs.graph,
            program_defs,
            public_surfaces: Some(public_surfaces),
            using_scope: Some(using_scope),
            extensions: Some(extensions),
            extension_interner: Some(extension_interner),
        },
    )
}

fn resolve_module_values_from_item_tree_inner(
    item_tree: &ModuleItemTree,
    inputs: ValueResolveInputs<'_>,
) -> ValueResolution {
    resolve_module_values_from_items(&item_tree.items, inputs)
}

fn resolve_module_values_from_items(
    items: &[ItemTreeNode],
    inputs: ValueResolveInputs<'_>,
) -> ValueResolution {
    let mut resolver = ValueResolver {
        defs: inputs.defs,
        graph: inputs.graph,
        program_defs: inputs.program_defs,
        public_surfaces: inputs.public_surfaces,
        using_scope: inputs.using_scope,
        extensions: inputs.extensions,
        extension_interner: inputs.extension_interner,
        node_names: HashMap::new(),
        node_qualified_values: HashMap::new(),
        node_builtin_associated_values: HashMap::new(),
        node_variant_enums: HashMap::new(),
        node_qualified_type_prefixes: HashMap::new(),
        node_builtins: HashMap::new(),
        diagnostics: Vec::new(),
    };
    for item in items {
        resolver.visit_item_tree_node(item);
    }
    ValueResolution {
        node_names: resolver.node_names,
        node_qualified_values: resolver.node_qualified_values,
        node_builtin_associated_values: resolver.node_builtin_associated_values,
        node_variant_enums: resolver.node_variant_enums,
        node_qualified_type_prefixes: resolver.node_qualified_type_prefixes,
        node_builtins: resolver.node_builtins,
        diagnostics: resolver.diagnostics,
    }
}

struct ValueResolveInputs<'a> {
    defs: &'a DefCollection,
    graph: Option<&'a ModuleGraph>,
    program_defs: ProgramDefsContext<'a>,
    public_surfaces: Option<&'a PublicSurfaces>,
    using_scope: Option<&'a ModuleUsingScope>,
    extensions: Option<&'a VisibleExtensionMethods>,
    extension_interner: Option<&'a TyInterner>,
}

struct ValueResolver<'a> {
    defs: &'a DefCollection,
    graph: Option<&'a ModuleGraph>,
    program_defs: ProgramDefsContext<'a>,
    public_surfaces: Option<&'a PublicSurfaces>,
    using_scope: Option<&'a ModuleUsingScope>,
    extensions: Option<&'a VisibleExtensionMethods>,
    extension_interner: Option<&'a TyInterner>,
    node_names: HashMap<NodeKey, ValueNameResolution>,
    node_qualified_values: HashMap<NodeKey, GlobalDefId>,
    node_builtin_associated_values: HashMap<NodeKey, BuiltinAssociatedValue>,
    node_variant_enums: HashMap<NodeKey, GlobalDefId>,
    node_qualified_type_prefixes: HashMap<NodeKey, GlobalDefId>,
    node_builtins: HashMap<NodeKey, BuiltinResolution>,
    diagnostics: Vec<Diagnostic>,
}

impl ValueResolver<'_> {
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

    fn direct_value_member(&self, module_id: ModuleId, name: &str) -> DirectMember<DefId> {
        let Some(target_defs) = self.defs_for_module(module_id) else {
            return DirectMember::Unloaded;
        };
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
    name: &'a str,
    span: Span,
    node_key: &'a NodeKey,
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
            ExprKind::Builtin { name, .. } => {
                let resolution = self.resolve_builtin(name, expr.span);
                self.node_builtins.insert(expr.node_key.clone(), resolution);
                walk_expr(self, expr);
            }
            ExprKind::TypeTarget { .. } => {
                walk_expr(self, expr);
            }
            ExprKind::Call { callee, args } => {
                if let ExprKind::Builtin { name, .. } = &callee.kind
                    && name == "asm"
                {
                    self.visit_expr(callee);
                    for arg in args {
                        self.visit_asm_config(arg);
                    }
                } else {
                    walk_expr(self, expr);
                }
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
}

impl<'a> ValueResolver<'a> {
    fn visit_item_tree_node(&mut self, item: &ItemTreeNode) {
        match &item.kind {
            ItemTreeNodeKind::Struct(item_struct) => {
                walk_where_clause(self, &item_struct.where_clause);
                for field in &item_struct.fields {
                    self.visit_type(&field.ty);
                }
            }
            ItemTreeNodeKind::Union(item_union) => {
                walk_where_clause(self, &item_union.where_clause);
                for field in &item_union.fields {
                    self.visit_type(&field.ty);
                }
            }
            ItemTreeNodeKind::Trait(item_trait) => {
                for supertrait in &item_trait.supertraits {
                    self.visit_type(supertrait);
                }
                walk_where_clause(self, &item_trait.where_clause);
                for method in &item_trait.methods {
                    self.visit_function(&method.function);
                }
            }
            ItemTreeNodeKind::Extend(extend) => {
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
            ItemTreeNodeKind::Module(_)
            | ItemTreeNodeKind::Using(_)
            | ItemTreeNodeKind::ComptimeIf(_) => {}
            ItemTreeNodeKind::TypeAlias(alias) => {
                walk_where_clause(self, &alias.where_clause);
                self.visit_type(&alias.ty);
            }
        }
    }

    fn visit_asm_config(&mut self, expr: &Expr) {
        let ExprKind::StructLiteral { fields } = &expr.kind else {
            self.visit_expr(expr);
            return;
        };
        for field in fields {
            match field.name.as_str() {
                "inputs" | "outputs" => self.visit_expr(&field.value),
                "code" | "clobbers" => {}
                _ => self.visit_expr(&field.value),
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
        let path_text = qualified_path_text(&segments);
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
        if let Some(module_id) = self.root_module_for_segment(segment.name) {
            self.insert_name(segment.node_key, ValueNameResolution::Module);
            return Some(ResolvedNamespace::Module(module_id));
        }
        if let Some(scope) = self.using_scope
            && let Some(module_id) = scope.lookup_module(segment.name)
        {
            self.insert_name(segment.node_key, ValueNameResolution::Module);
            return Some(ResolvedNamespace::Module(module_id));
        }
        if let Some(def_id) = self.defs.module_scope.types.get(segment.name) {
            return Some(ResolvedNamespace::Type(GlobalDefId {
                module_id: self.defs.module_id,
                def_id,
            }));
        }
        if let Some(scope) = self.using_scope
            && let Some(entry) = scope.lookup_type(segment.name)
        {
            let type_id = GlobalDefId {
                module_id: entry.target_module,
                def_id: entry.target_def_id,
            };
            self.insert_name(segment.node_key, ValueNameResolution::External(type_id));
            self.insert_qualified_type_prefix(segment.node_key, type_id);
            return Some(ResolvedNamespace::Type(type_id));
        }
        if let Some(primitive) = PrimitiveTy::from_name(segment.name) {
            return Some(ResolvedNamespace::Primitive(primitive));
        }
        None
    }

    fn root_module_for_segment(&self, name: &str) -> Option<ModuleId> {
        let graph = self.graph()?;
        match name {
            "self" => Some(self.defs.module_id),
            "super" => graph.get(self.defs.module_id)?.parent,
            ROOT_MODULE_MAP_NAME => Some(graph.root()),
            PACKAGE_MODULE_MAP_NAME => graph.current_package_root(self.defs.module_id),
            package => graph
                .get(self.defs.module_id)
                .and_then(|node| node.children.get(package).copied())
                .or_else(|| graph.package_root(package)),
        }
    }

    fn resolve_child_namespace(
        &mut self,
        namespace: ResolvedNamespace,
        segment: PathSegment<'_>,
    ) -> Option<ResolvedNamespace> {
        match namespace {
            ResolvedNamespace::Module(module_id) => {
                if let Some(surfaces) = self.public_surfaces
                    && let Some(surface) = surfaces.get(module_id)
                {
                    if let Some(child_module) = surface.lookup_module(segment.name) {
                        return Some(ResolvedNamespace::Module(child_module));
                    }
                    if let Some(item) = surface.lookup_type(segment.name) {
                        return Some(ResolvedNamespace::Type(GlobalDefId {
                            module_id: item.target_module,
                            def_id: item.target_def_id,
                        }));
                    }
                }
                if let Some((child_module, visibility)) =
                    self.child_module_declaration(module_id, segment.name)
                {
                    if self.module_declaration_visible(module_id, visibility) {
                        return Some(ResolvedNamespace::Module(child_module));
                    }
                    self.diagnostics.push(Diagnostic::user_error_at(
                        "E0201",
                        segment.span,
                        format!("module namespace `{}` is private", segment.name),
                    ));
                    return None;
                }
                match self.direct_type_member(module_id, segment.name) {
                    DirectMember::Visible(def_id) => {
                        Some(ResolvedNamespace::Type(GlobalDefId { module_id, def_id }))
                    }
                    DirectMember::Private => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            "E0201",
                            segment.span,
                            format!("type `{}` is private", segment.name),
                        ));
                        None
                    }
                    DirectMember::Missing => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            "E0201",
                            segment.span,
                            format!("unknown namespace `{}`", segment.name),
                        ));
                        None
                    }
                    DirectMember::Unloaded => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            "E0201",
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
        node_key: &NodeKey,
        module_id: ModuleId,
        name: PathSegment<'_>,
        path_text: &str,
    ) {
        if let Some(surfaces) = self.public_surfaces
            && let Some(surface) = surfaces.get(module_id)
        {
            if let Some(item) = surface.lookup_value(name.name) {
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
            if let Some(item) = surface.lookup_type(name.name) {
                self.insert_qualified_type_prefix(
                    node_key,
                    GlobalDefId {
                        module_id: item.target_module,
                        def_id: item.target_def_id,
                    },
                );
                return;
            }
            if surface.lookup_module(name.name).is_some() {
                return;
            }
        }
        if let Some((_child_module, visibility)) =
            self.child_module_declaration(module_id, name.name)
        {
            if self.module_declaration_visible(module_id, visibility) {
                return;
            }
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0201",
                span,
                format!("module namespace `{}` is private", name.name),
            ));
            return;
        }
        match self.direct_type_member(module_id, name.name) {
            DirectMember::Visible(def_id) => {
                self.insert_qualified_type_prefix(node_key, GlobalDefId { module_id, def_id });
                return;
            }
            DirectMember::Private => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    "E0201",
                    span,
                    format!("type `{path_text}` is private"),
                ));
                return;
            }
            DirectMember::Missing => {}
            DirectMember::Unloaded => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    "E0201",
                    span,
                    "module namespace refers to an unloaded module",
                ));
                return;
            }
        }
        let def_id = match self.direct_value_member(module_id, name.name) {
            DirectMember::Visible(def_id) => def_id,
            DirectMember::Private => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    "E0201",
                    span,
                    format!("value `{path_text}` is private"),
                ));
                return;
            }
            DirectMember::Missing => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    "E0201",
                    span,
                    format!("unknown value `{}`", name.name),
                ));
                return;
            }
            DirectMember::Unloaded => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    "E0201",
                    span,
                    "module namespace refers to an unloaded module",
                ));
                return;
            }
        };
        let Some(target_defs) = self.defs_for_module(module_id) else {
            return;
        };
        let Some(def) = target_defs.defs.get(def_id) else {
            return;
        };
        if matches!(
            def.kind,
            DefKind::Function | DefKind::Global | DefKind::Comptime
        ) {
            self.insert_qualified_value(node_key, GlobalDefId { module_id, def_id });
        }
    }

    fn resolve_type_qualified_value(
        &mut self,
        node_key: &NodeKey,
        type_id: GlobalDefId,
        name: PathSegment<'_>,
    ) {
        let Some(target_defs) = self.defs_for_module(type_id.module_id) else {
            return;
        };
        let Some(def) = target_defs.defs.get(type_id.def_id) else {
            return;
        };
        if def.kind == DefKind::Enum
            && let Some(enum_scope) = target_defs.scopes.enum_members.get(&type_id.def_id)
            && let Some(variant_def_id) = enum_scope.variants.get(name.name)
        {
            let variant_id = GlobalDefId {
                module_id: type_id.module_id,
                def_id: variant_def_id,
            };
            self.insert_qualified_value(node_key, variant_id);
            self.insert_variant_enum(node_key, type_id);
            return;
        }
        let Some(target_ty) = self.nominal_extension_target_ty(type_id) else {
            return;
        };
        self.resolve_associated_value(node_key, target_ty, name.name);
    }

    fn resolve_primitive_qualified_value(
        &mut self,
        node_key: &NodeKey,
        primitive: PrimitiveTy,
        name: PathSegment<'_>,
    ) {
        let Some(interner) = self.extension_interner else {
            return;
        };
        if let Some(value) = primitive_associated_value(primitive, name.name) {
            self.insert_builtin_associated_value(node_key, value);
            return;
        }
        let target_ty = interner.primitive(primitive);
        self.resolve_associated_value(node_key, target_ty, name.name);
    }

    fn resolve_associated_value(
        &mut self,
        node_key: &NodeKey,
        target_ty: nia_ids::InternedTyId,
        name: &str,
    ) {
        let Some(extensions) = self.extensions else {
            return;
        };
        if let Some(value) = extensions.associated_value(target_ty, name) {
            self.insert_qualified_value(node_key, value.def_id);
        }
    }

    fn nominal_extension_target_ty(&self, type_id: GlobalDefId) -> Option<nia_ids::InternedTyId> {
        let interner = self.extension_interner?;
        interner.iter().find_map(|(ty, kind)| match kind {
            TyKind::Nominal { def_id, args } if *def_id == type_id && args.is_empty() => Some(ty),
            _ => None,
        })
    }

    fn resolve_ident(&mut self, name: &str, node_key: &NodeKey) -> ValueNameResolution {
        if let Some(def_id) = self.defs.module_scope.values.get(name) {
            let Some(def) = self.defs.defs.get(def_id) else {
                return ValueNameResolution::Error;
            };
            if matches!(
                def.kind,
                DefKind::Function | DefKind::Global | DefKind::Comptime
            ) {
                return ValueNameResolution::Def(def_id);
            }
        }

        if let Some(scope) = self.using_scope
            && let Some(entry) = scope.lookup_value(name)
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

        if let Some(scope) = self.using_scope
            && let Some(entry) = scope.lookup_type(name)
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
            .is_some_and(|scope| scope.has_unresolved_name(name))
        {
            return ValueNameResolution::Error;
        }

        // Local bindings and parameters are resolved by nia-local-resolve.
        ValueNameResolution::LocalDeferred
    }

    fn resolve_builtin(&mut self, name: &str, span: Span) -> BuiltinResolution {
        match name {
            "builtin" => BuiltinResolution::Builtin,
            "error" => BuiltinResolution::ComptimeError,
            "trap" => BuiltinResolution::Trap,
            "size" => BuiltinResolution::SizeOf,
            "align" => BuiltinResolution::AlignOf,
            "asm" => BuiltinResolution::Asm,
            "memcpy" => BuiltinResolution::MemCopy,
            "memmove" => BuiltinResolution::MemMove,
            "memset" => BuiltinResolution::MemSet,
            "load_unaligned" => BuiltinResolution::LoadUnaligned,
            "splat" => BuiltinResolution::Splat,
            "extract" => BuiltinResolution::Extract,
            "insert" => BuiltinResolution::Insert,
            "bitmask" => BuiltinResolution::Bitmask,
            "ctz" => BuiltinResolution::Ctz,
            "clz" => BuiltinResolution::Clz,
            "popcount" => BuiltinResolution::Popcount,
            "atomic_load" => BuiltinResolution::AtomicLoad,
            "atomic_store" => BuiltinResolution::AtomicStore,
            "atomic_rmw" => BuiltinResolution::AtomicRmw,
            "cmpxchg_strong" => BuiltinResolution::CmpxchgStrong,
            "cmpxchg_weak" => BuiltinResolution::CmpxchgWeak,
            "fence" => BuiltinResolution::Fence,
            _ => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    "E0201",
                    span,
                    format!("unknown builtin `@{name}`"),
                ));
                BuiltinResolution::Reserved
            }
        }
    }

    fn defs_for_module(&self, module_id: ModuleId) -> Option<&DefCollection> {
        if module_id == self.defs.module_id {
            Some(self.defs)
        } else {
            self.program_defs.defs?.get(&module_id)
        }
    }

    fn insert_name(&mut self, node_key: &NodeKey, resolution: ValueNameResolution) {
        self.node_names.insert(node_key.clone(), resolution);
    }

    fn insert_qualified_value(&mut self, node_key: &NodeKey, global_id: GlobalDefId) {
        self.node_qualified_values
            .insert(node_key.clone(), global_id);
    }

    fn insert_builtin_associated_value(
        &mut self,
        node_key: &NodeKey,
        value: BuiltinAssociatedValue,
    ) {
        self.node_builtin_associated_values
            .insert(node_key.clone(), value);
    }

    fn insert_variant_enum(&mut self, node_key: &NodeKey, enum_id: GlobalDefId) {
        self.node_variant_enums.insert(node_key.clone(), enum_id);
    }

    fn insert_qualified_type_prefix(&mut self, node_key: &NodeKey, type_id: GlobalDefId) {
        self.node_qualified_type_prefixes
            .insert(node_key.clone(), type_id);
    }
}

fn qualified_path_segments(expr: &Expr) -> Option<Vec<PathSegment<'_>>> {
    fn collect<'a>(expr: &'a Expr, segments: &mut Vec<PathSegment<'a>>) -> Option<()> {
        match &expr.kind {
            ExprKind::Ident(name) => {
                segments.push(PathSegment {
                    name: name.as_str(),
                    span: expr.span,
                    node_key: &expr.node_key,
                });
                Some(())
            }
            ExprKind::Qualified { lhs, name } => {
                collect(lhs, segments)?;
                segments.push(PathSegment {
                    name: name.as_str(),
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

fn qualified_path_text(segments: &[PathSegment<'_>]) -> String {
    segments
        .iter()
        .map(|segment| segment.name)
        .collect::<Vec<_>>()
        .join("::")
}

fn primitive_associated_value(
    primitive: PrimitiveTy,
    name: &str,
) -> Option<BuiltinAssociatedValue> {
    let kind = match name {
        "MIN" => PrimitiveIntLimit::Min,
        "MAX" => PrimitiveIntLimit::Max,
        _ => return None,
    };
    supports_primitive_int_limit(primitive)
        .then_some(BuiltinAssociatedValue::PrimitiveIntLimit { primitive, kind })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{ModuleId, collect_module_defs, collect_module_defs_from_active_item_tree};
    use nia_item_tree::ModuleItemTree;
    use nia_parser::parse_module;

    #[test]
    fn resolves_module_value_names_and_defers_locals() {
        let (module, errors) = parse_module(
            r#"
var counter = 0;

fn add(a: i32, b: i32) i32 {
    a + b + counter
}

fn main() i32 {
    var local = add(counter, 1);
    local
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
        let resolved = resolve_module_values(&module, &defs);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        assert!(
            resolved
                .node_names
                .values()
                .any(|resolution| matches!(resolution, ValueNameResolution::Def(_)))
        );
        assert!(
            resolved
                .node_names
                .values()
                .any(|resolution| matches!(resolution, ValueNameResolution::LocalDeferred))
        );
    }

    #[test]
    fn validates_builtin_names_only() {
        let (module, errors) = parse_module(
            r#"
fn main() usize {
    var a = @size[usize]();
    var b = @align[usize]();
    var c = @unknown[usize]();
    comptime let d: usize = @error("bad");
    @trap();
    a + b + c
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_values(&module, &defs);
        assert_eq!(resolved.diagnostics.len(), 1);
        assert!(
            resolved.diagnostics[0]
                .summary
                .contains("unknown builtin `@unknown`")
        );
        assert!(
            resolved
                .node_builtins
                .values()
                .any(|builtin| matches!(builtin, BuiltinResolution::SizeOf))
        );
        assert!(
            resolved
                .node_builtins
                .values()
                .any(|builtin| matches!(builtin, BuiltinResolution::AlignOf))
        );
        assert!(
            resolved
                .node_builtins
                .values()
                .any(|builtin| matches!(builtin, BuiltinResolution::Trap))
        );
        assert!(
            resolved
                .node_builtins
                .values()
                .any(|builtin| matches!(builtin, BuiltinResolution::ComptimeError))
        );
    }

    #[test]
    fn resolves_values_from_active_item_tree_only() {
        let (module, errors) = parse_module(
            r#"
comptime if false {
    fn skipped() usize {
        @unknown[usize]()
    }
} else {
    fn selected() usize {
        @size[usize]()
    }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let tree = ModuleItemTree::from_module(&module);
        let active = tree.active_items(&mut BoolResolver(false)).unwrap();
        let defs = collect_module_defs_from_active_item_tree(ModuleId(0), &active);
        assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
        let resolved = resolve_module_values_from_active_item_tree(
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
                .node_builtins
                .values()
                .any(|builtin| matches!(builtin, BuiltinResolution::SizeOf))
        );
    }

    struct BoolResolver(bool);

    impl nia_item_tree::ComptimeBranchResolver for BoolResolver {
        fn resolve_comptime_if(
            &mut self,
            span: Span,
            _cond: &nia_ast::Expr,
        ) -> Result<nia_item_tree::ComptimeBranch, nia_item_tree::ItemTreeError> {
            let _ = span;
            Ok(if self.0 {
                nia_item_tree::ComptimeBranch::Then
            } else {
                nia_item_tree::ComptimeBranch::Else
            })
        }
    }
}
