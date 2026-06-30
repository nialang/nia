// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{
    ArrayLen, BindingItem, BindingStmt, Block, Expr, ExprKind, FunctionItem, IndexArg, Module,
    Pattern, PatternKind, Stmt, StmtKind, SwitchArmBody, SwitchPattern, SwitchPatternKind, TypeArg,
    TypeKind, TypeRef,
};
use nia_defs::DefCollection;
use nia_diagnostic::{Diagnostic, codes};
pub use nia_ids::LocalId;
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNode, ItemTreeNodeKind, ModuleItemTree};
use nia_node_id::VersionedNodeKey;
use nia_span::Span;
use nia_value_resolve::{ValueNameResolution, ValueResolution};

#[derive(Debug, Clone, PartialEq)]
pub struct LocalResolution {
    pub locals: LocalMap,
    pub node_local_defs: HashMap<VersionedNodeKey, LocalId>,
    pub node_uses: HashMap<VersionedNodeKey, LocalUse>,
    pub diagnostics: Vec<Diagnostic>,
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
    pub name: String,
    pub kind: LocalKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalKind {
    Param,
    Binding,
    ConstBinding,
    ComptimeBinding,
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
    resolve_module_locals_from_items(&item_tree.items, defs, values)
}

pub fn resolve_module_locals_with_origins(
    module: &Module,
    defs: &DefCollection,
    values: &ValueResolution,
    _source_version: Option<nia_source::SourceVersion>,
    _origins: &nia_node_id::NodeOriginTable,
) -> LocalResolution {
    let item_tree = ModuleItemTree::from_module(module);
    resolve_module_locals_from_items(&item_tree.items, defs, values)
}

pub fn resolve_module_locals_from_item_tree(
    item_tree: &ModuleItemTree,
    defs: &DefCollection,
    values: &ValueResolution,
) -> LocalResolution {
    resolve_module_locals_from_items(&item_tree.items, defs, values)
}

pub fn resolve_module_locals_from_active_item_tree_with_origins(
    item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    values: &ValueResolution,
    _source_version: Option<nia_source::SourceVersion>,
    _origins: &nia_node_id::NodeOriginTable,
) -> LocalResolution {
    resolve_module_locals_from_items(&item_tree.items, defs, values)
}

pub fn resolve_module_locals_from_filtered_active_item_tree_with_origins(
    filtered_item_tree: &ActiveModuleItemTree,
    full_item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    values: &ValueResolution,
    _source_version: Option<nia_source::SourceVersion>,
    _origins: &nia_node_id::NodeOriginTable,
) -> LocalResolution {
    resolve_module_locals_from_filtered_items(
        &filtered_item_tree.items,
        &full_item_tree.items,
        defs,
        values,
    )
}

pub fn resolve_module_locals_from_item_tree_with_origins(
    item_tree: &ModuleItemTree,
    defs: &DefCollection,
    values: &ValueResolution,
    _source_version: Option<nia_source::SourceVersion>,
    _origins: (),
) -> LocalResolution {
    resolve_module_locals_from_items(&item_tree.items, defs, values)
}

fn resolve_module_locals_from_filtered_items(
    filtered_items: &[ItemTreeNode],
    full_items: &[ItemTreeNode],
    defs: &DefCollection,
    values: &ValueResolution,
) -> LocalResolution {
    let allocated = LocalDefinitionAllocator::allocate_items(full_items);
    let mut resolver = LocalResolver {
        defs,
        values,
        locals: allocated.locals,
        node_local_defs: allocated.node_local_defs.clone(),
        node_uses: HashMap::new(),
        diagnostics: Vec::new(),
        scopes: Vec::new(),
        definition_ids: Some(allocated.node_local_defs),
    };
    resolver.resolve_items(filtered_items);
    LocalResolution {
        locals: resolver.locals,
        node_local_defs: resolver.node_local_defs,
        node_uses: resolver.node_uses,
        diagnostics: resolver.diagnostics,
    }
}

fn resolve_module_locals_from_items(
    items: &[ItemTreeNode],
    defs: &DefCollection,
    values: &ValueResolution,
) -> LocalResolution {
    let mut resolver = LocalResolver {
        defs,
        values,
        locals: LocalMap::default(),
        node_local_defs: HashMap::new(),
        node_uses: HashMap::new(),
        diagnostics: Vec::new(),
        scopes: Vec::new(),
        definition_ids: None,
    };
    resolver.resolve_items(items);
    LocalResolution {
        locals: resolver.locals,
        node_local_defs: resolver.node_local_defs,
        node_uses: resolver.node_uses,
        diagnostics: resolver.diagnostics,
    }
}

struct LocalResolver<'a> {
    defs: &'a DefCollection,
    values: &'a ValueResolution,
    locals: LocalMap,
    node_local_defs: HashMap<VersionedNodeKey, LocalId>,
    node_uses: HashMap<VersionedNodeKey, LocalUse>,
    diagnostics: Vec<Diagnostic>,
    scopes: Vec<Scope>,
    definition_ids: Option<HashMap<VersionedNodeKey, LocalId>>,
}

#[derive(Debug, Clone, Copy)]
struct ScopedLocal {
    id: LocalId,
    span: Span,
}

#[derive(Debug, Clone, Copy)]
struct ScopedStatic {
    id: nia_ids::GlobalDefId,
    span: Span,
}

#[derive(Debug, Clone, Default)]
struct Scope {
    locals: HashMap<String, ScopedLocal>,
    statics: HashMap<String, ScopedStatic>,
}

#[derive(Default)]
struct LocalDefinitionAllocator {
    locals: LocalMap,
    node_local_defs: HashMap<VersionedNodeKey, LocalId>,
}

impl LocalDefinitionAllocator {
    fn allocate_items(items: &[ItemTreeNode]) -> Self {
        let mut allocator = Self::default();
        for item in items {
            allocator.allocate_item_tree_node(item);
        }
        allocator
    }

    fn allocate_item_tree_node(&mut self, item: &ItemTreeNode) {
        match &item.kind {
            ItemTreeNodeKind::Function(function) => self.allocate_function(function),
            ItemTreeNodeKind::Trait(item_trait) => {
                for method in &item_trait.methods {
                    self.allocate_function(&method.function);
                }
            }
            ItemTreeNodeKind::Extend(extend) => {
                for associated_value in &extend.associated_values {
                    if let Some(value) = &associated_value.binding.value {
                        self.allocate_expr(value);
                    }
                }
                for method in &extend.methods {
                    self.allocate_function(&method.function);
                }
            }
            ItemTreeNodeKind::Enum(item_enum) => {
                for variant in &item_enum.variants {
                    if let Some(value) = &variant.value {
                        self.allocate_expr(value);
                    }
                }
            }
            ItemTreeNodeKind::Binding(binding) => {
                if let Some(value) = &binding.value {
                    self.allocate_expr(value);
                }
            }
            ItemTreeNodeKind::Module(_)
            | ItemTreeNodeKind::Using(_)
            | ItemTreeNodeKind::Struct(_)
            | ItemTreeNodeKind::Union(_)
            | ItemTreeNodeKind::TypeAlias(_) => {}
        }
    }

    fn allocate_function(&mut self, function: &FunctionItem) {
        for param in &function.params {
            if let Some(name) = &param.name {
                self.allocate_definition(
                    name,
                    LocalKind::Param,
                    param.span,
                    param.node_key.clone(),
                );
            }
        }
        if let Some(body) = &function.body {
            self.allocate_block(body);
        }
    }

    fn allocate_block(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.allocate_stmt(stmt);
        }
        if let Some(tail) = &block.tail {
            self.allocate_expr(tail);
        }
    }

    fn allocate_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Binding(binding) => {
                self.allocate_binding(stmt.span, binding, stmt.node_key.clone())
            }
            StmtKind::Static(binding) => {
                if let Some(value) = &binding.value {
                    self.allocate_expr(value);
                }
            }
            StmtKind::Expr(expr) | StmtKind::Return(Some(expr)) | StmtKind::Defer(expr) => {
                self.allocate_expr(expr);
            }
            StmtKind::ForIn(for_stmt) => {
                self.allocate_expr(&for_stmt.iter);
                self.allocate_pattern(&for_stmt.pattern, LocalKind::ConstBinding);
                self.allocate_block(&for_stmt.body);
            }
            StmtKind::While(while_stmt) => {
                self.allocate_expr(&while_stmt.cond);
                self.allocate_block(&while_stmt.body);
            }
            StmtKind::Loop(loop_stmt) => self.allocate_block(&loop_stmt.body),
            StmtKind::Using(_) | StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {}
        }
    }

    fn allocate_binding(
        &mut self,
        span: Span,
        binding: &BindingStmt,
        fallback_key: VersionedNodeKey,
    ) {
        if let Some(value) = &binding.value {
            self.allocate_expr(value);
        }
        let _ = fallback_key;
        let default_kind = if binding.is_comptime {
            LocalKind::ComptimeBinding
        } else if binding.is_mutable {
            LocalKind::Binding
        } else {
            LocalKind::ConstBinding
        };
        self.allocate_pattern_with_span(&binding.pattern, default_kind, span);
    }

    fn allocate_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Ident(_)
            | ExprKind::TypeTarget { .. }
            | ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::ByteString(_)
            | ExprKind::Char(_)
            | ExprKind::ByteChar(_)
            | ExprKind::Raw(_)
            | ExprKind::Bool(_)
            | ExprKind::Null
            | ExprKind::Underscore
            | ExprKind::Error => {}
            ExprKind::BracketSuffix { callee, args } => {
                self.allocate_expr(callee);
                for arg in args {
                    if let Some(expr) = &arg.expr {
                        self.allocate_expr(expr);
                    }
                }
            }
            ExprKind::ArrayLiteral { elems } | ExprKind::TypedArrayLiteral { elems, .. } => {
                self.allocate_array_elements(elems);
            }
            ExprKind::StructLiteral { fields } | ExprKind::TypedStructLiteral { fields, .. } => {
                for field in fields {
                    self.allocate_expr(&field.value);
                }
            }
            ExprKind::Unary { expr, .. }
            | ExprKind::OptionalSome { expr }
            | ExprKind::ErrorOk { expr }
            | ExprKind::ErrorErr { expr }
            | ExprKind::Try { expr }
            | ExprKind::Cast { expr, .. } => self.allocate_expr(expr),
            ExprKind::Binary { lhs, rhs, .. } | ExprKind::Assign { lhs, rhs, .. } => {
                self.allocate_expr(lhs);
                self.allocate_expr(rhs);
            }
            ExprKind::Call { callee, args } => {
                self.allocate_expr(callee);
                for arg in args {
                    self.allocate_expr(arg);
                }
            }
            ExprKind::Qualified { lhs, .. } | ExprKind::Field { lhs, .. } => {
                self.allocate_expr(lhs);
            }
            ExprKind::Index { lhs, index } => {
                self.allocate_expr(lhs);
                match index {
                    IndexArg::Expr(index) => self.allocate_expr(index),
                    IndexArg::Range(range) => {
                        if let Some(start) = &range.start {
                            self.allocate_expr(start);
                        }
                        if let Some(end) = &range.end {
                            self.allocate_expr(end);
                        }
                    }
                }
            }
            ExprKind::Range(range) => {
                if let Some(start) = &range.start {
                    self.allocate_expr(start);
                }
                if let Some(end) = &range.end {
                    self.allocate_expr(end);
                }
            }
            ExprKind::Block(block) => self.allocate_block(block),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.allocate_expr(cond);
                self.allocate_block(then_branch);
                if let Some(else_branch) = else_branch {
                    self.allocate_expr(else_branch);
                }
            }
            ExprKind::IfPattern(if_pattern) => {
                self.allocate_expr(&if_pattern.target);
                for arm in &if_pattern.arms {
                    self.allocate_pattern(&arm.pattern, LocalKind::ConstBinding);
                    self.allocate_block(&arm.body);
                }
                if let Some(else_branch) = &if_pattern.else_branch {
                    self.allocate_expr(else_branch);
                }
            }
            ExprKind::Switch(switch) => {
                self.allocate_expr(&switch.target);
                for arm in &switch.arms {
                    for pattern in &arm.patterns {
                        self.allocate_switch_pattern(pattern);
                    }
                    match &arm.body {
                        SwitchArmBody::Expr(expr) => self.allocate_expr(expr),
                        SwitchArmBody::Stmt(stmt) => self.allocate_stmt(stmt),
                        SwitchArmBody::Block(block) => self.allocate_block(block),
                    }
                }
            }
        }
    }

    fn allocate_array_elements(&mut self, elems: &nia_ast::ArrayElements) {
        match elems {
            nia_ast::ArrayElements::List(elems) => {
                for elem in elems {
                    self.allocate_expr(elem);
                }
            }
            nia_ast::ArrayElements::Repeat { value, count } => {
                self.allocate_expr(value);
                self.allocate_expr(count);
            }
        }
    }

    fn allocate_switch_pattern(&mut self, pattern: &SwitchPattern) {
        match &pattern.kind {
            SwitchPatternKind::Wildcard => {}
            SwitchPatternKind::Expr(expr) => self.allocate_expr(expr),
            SwitchPatternKind::Range { start, end, .. } => {
                self.allocate_expr(start);
                self.allocate_expr(end);
            }
        }
    }

    fn allocate_pattern(&mut self, pattern: &Pattern, binding_kind: LocalKind) {
        self.allocate_pattern_with_span(pattern, binding_kind, pattern.span);
    }

    fn allocate_pattern_with_span(
        &mut self,
        pattern: &Pattern,
        binding_kind: LocalKind,
        binding_span: Span,
    ) {
        match &pattern.kind {
            PatternKind::Wildcard | PatternKind::OptionalNull => {}
            PatternKind::Bind {
                name,
                node_key,
                is_mutable,
            } => {
                let kind = if matches!(binding_kind, LocalKind::ComptimeBinding) {
                    LocalKind::ComptimeBinding
                } else if *is_mutable || matches!(binding_kind, LocalKind::Binding) {
                    LocalKind::Binding
                } else {
                    LocalKind::ConstBinding
                };
                self.allocate_definition(name, kind, binding_span, node_key.clone());
            }
            PatternKind::Pointer(pattern) | PatternKind::MutPointer(pattern) => {
                self.allocate_pattern_with_span(pattern, binding_kind, binding_span)
            }
            PatternKind::OptionalSome(pattern)
            | PatternKind::ErrorOk(pattern)
            | PatternKind::ErrorErr(pattern) => {
                self.allocate_pattern_with_span(pattern, binding_kind, binding_span)
            }
            PatternKind::Expr(pattern) => self.allocate_expr(pattern),
            PatternKind::Range { start, end, .. } => {
                self.allocate_expr(start);
                self.allocate_expr(end);
            }
        }
    }

    fn allocate_definition(
        &mut self,
        name: &str,
        kind: LocalKind,
        span: Span,
        node_key: VersionedNodeKey,
    ) {
        let id = self.locals.push(Local {
            name: name.to_string(),
            kind,
            span,
        });
        self.node_local_defs.insert(node_key, id);
    }
}

impl<'a> LocalResolver<'a> {
    fn resolve_items(&mut self, items: &[ItemTreeNode]) {
        for item in items {
            self.resolve_item_tree_node(item);
        }
    }

    fn resolve_item_tree_node(&mut self, item: &ItemTreeNode) {
        match &item.kind {
            ItemTreeNodeKind::Function(function) => self.resolve_function(function),
            ItemTreeNodeKind::Trait(item_trait) => {
                self.resolve_where_clause(&item_trait.where_clause);
                for method in &item_trait.methods {
                    self.resolve_function(&method.function);
                }
            }
            ItemTreeNodeKind::Extend(extend) => {
                self.resolve_type(&extend.target);
                if let Some(trait_ref) = &extend.trait_ref {
                    self.resolve_type(trait_ref);
                }
                self.resolve_where_clause(&extend.where_clause);
                for associated_value in &extend.associated_values {
                    if let Some(ty) = &associated_value.binding.ty {
                        self.resolve_type(ty);
                    }
                    if let Some(value) = &associated_value.binding.value {
                        self.resolve_expr(value);
                    }
                }
                for method in &extend.methods {
                    self.resolve_function(&method.function);
                }
            }
            ItemTreeNodeKind::Enum(item_enum) => {
                for variant in &item_enum.variants {
                    if let Some(value) = &variant.value {
                        self.resolve_expr(value);
                    }
                }
            }
            ItemTreeNodeKind::Binding(binding) => {
                if let Some(ty) = &binding.ty {
                    self.resolve_type(ty);
                }
                if let Some(value) = &binding.value {
                    self.resolve_expr(value);
                }
            }
            ItemTreeNodeKind::Module(_)
            | ItemTreeNodeKind::Using(_)
            | ItemTreeNodeKind::Struct(_)
            | ItemTreeNodeKind::Union(_)
            | ItemTreeNodeKind::TypeAlias(_) => {}
        }
    }

    fn resolve_function(&mut self, function: &FunctionItem) {
        self.push_scope();
        self.resolve_where_clause(&function.where_clause);
        for param in &function.params {
            if let Some(ty) = &param.ty {
                self.resolve_type(ty);
            }
            if let Some(name) = &param.name {
                self.define(
                    name,
                    LocalKind::Param,
                    param.span,
                    param.node_key.clone(),
                    "duplicate parameter name",
                );
            }
        }
        if let Some(return_type) = &function.return_type {
            self.resolve_type(return_type);
        }
        if let Some(body) = &function.body {
            self.resolve_block(body);
        }
        self.pop_scope();
    }

    fn resolve_block(&mut self, block: &Block) {
        self.push_scope();
        for stmt in &block.stmts {
            self.resolve_stmt(stmt);
        }
        if let Some(tail) = &block.tail {
            self.resolve_expr(tail);
        }
        self.pop_scope();
    }

    fn resolve_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Binding(binding) => {
                self.resolve_binding(stmt.span, binding, stmt.node_key.clone());
            }
            StmtKind::Static(binding) => {
                self.resolve_static(stmt.span, binding);
            }
            StmtKind::Using(_) => {
                // Block-scope `using` is handled by a later resolution pass; nothing local to bind.
            }
            StmtKind::Expr(expr) | StmtKind::Defer(expr) => self.resolve_expr(expr),
            StmtKind::Return(value) => {
                if let Some(value) = value {
                    self.resolve_expr(value);
                }
            }
            StmtKind::Break | StmtKind::Continue => {}
            StmtKind::ForIn(for_stmt) => {
                self.resolve_expr(&for_stmt.iter);
                self.push_scope();
                self.resolve_pattern(
                    &for_stmt.pattern,
                    LocalKind::ConstBinding,
                    "duplicate local binding",
                );
                self.resolve_block(&for_stmt.body);
                self.pop_scope();
            }
            StmtKind::While(while_stmt) => {
                self.resolve_expr(&while_stmt.cond);
                self.resolve_block(&while_stmt.body);
            }
            StmtKind::Loop(loop_stmt) => self.resolve_block(&loop_stmt.body),
        }
    }

    fn resolve_binding(
        &mut self,
        span: Span,
        binding: &BindingStmt,
        fallback_key: VersionedNodeKey,
    ) {
        if let Some(ty) = &binding.ty {
            self.resolve_type(ty);
        }
        if let Some(value) = &binding.value {
            self.resolve_expr(value);
        }
        let _ = fallback_key;
        let default_kind = if binding.is_comptime {
            LocalKind::ComptimeBinding
        } else if binding.is_mutable {
            LocalKind::Binding
        } else {
            LocalKind::ConstBinding
        };
        self.resolve_pattern_with_span(
            &binding.pattern,
            default_kind,
            span,
            "duplicate local binding",
        );
    }

    fn resolve_static(&mut self, span: Span, binding: &BindingItem) {
        if let Some(ty) = &binding.ty {
            self.resolve_type(ty);
        }
        if let Some(value) = &binding.value {
            self.resolve_expr(value);
        }
        let Some(def_id) = self.defs.def_nodes.get(&binding.node_key).filter(|def_id| {
            self.defs
                .defs
                .get(*def_id)
                .is_some_and(|def| def.kind == nia_defs::DefKind::Global)
        }) else {
            self.diagnostics.push(
                Diagnostic::internal_error(
                    codes::LOCAL_RESOLVER_SCOPE,
                    "local static definition has no global definition id",
                )
                .primary(
                    span,
                    "local static was not registered by definition collection",
                )
                .debug("name", binding.name.clone())
                .debug("node_key", binding.node_key.clone())
                .finish(),
            );
            return;
        };
        self.define_static(
            &binding.name,
            nia_ids::GlobalDefId {
                module_id: self.defs.module_id,
                def_id,
            },
            span,
            "duplicate local static binding",
        );
    }

    fn resolve_type(&mut self, ty: &TypeRef) {
        match &ty.kind {
            TypeKind::Error
            | TypeKind::SelfType
            | TypeKind::Void
            | TypeKind::Never
            | TypeKind::Infer => {}
            TypeKind::Path { segments } => {
                for segment in segments {
                    for arg in &segment.args {
                        if let TypeArg::Type(ty) = arg {
                            self.resolve_type(ty);
                        }
                    }
                }
            }
            TypeKind::Projection { ty, trait_ref, .. } => {
                self.resolve_type(ty);
                self.resolve_type(trait_ref);
            }
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePointer { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::SlicePointee { elem } => {
                self.resolve_type(elem);
            }
            TypeKind::Array { len, elem } => {
                if let ArrayLen::Expr(expr) = len {
                    self.resolve_expr(expr);
                }
                self.resolve_type(elem);
            }
            TypeKind::Range { start, end, .. } => {
                if let Some(start) = start {
                    self.resolve_type(start);
                }
                if let Some(end) = end {
                    self.resolve_type(end);
                }
            }
            TypeKind::FunctionPointer {
                params,
                return_type,
                ..
            } => {
                for param in params {
                    self.resolve_type(param);
                }
                if let Some(return_type) = return_type {
                    self.resolve_type(return_type);
                }
            }
            TypeKind::Optional { elem } => self.resolve_type(elem),
            TypeKind::ErrorUnion { error, value } => {
                self.resolve_type(error);
                self.resolve_type(value);
            }
        }
    }

    fn resolve_where_clause(&mut self, clause: &nia_ast::WhereClause) {
        for predicate in &clause.predicates {
            self.resolve_type(&predicate.ty);
            for bound in &predicate.bounds {
                self.resolve_type(bound);
            }
        }
    }

    fn resolve_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Ident(name) => {
                self.resolve_ident(name, expr.node_key.clone());
            }
            ExprKind::TypeTarget { .. }
            | ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::ByteString(_)
            | ExprKind::Char(_)
            | ExprKind::ByteChar(_)
            | ExprKind::Raw(_)
            | ExprKind::Bool(_)
            | ExprKind::Null
            | ExprKind::Underscore
            | ExprKind::Error => {}
            ExprKind::BracketSuffix { callee, args } => {
                self.resolve_callee(callee);
                if self.should_resolve_expr_bracket_args(callee, args) {
                    for arg in args {
                        if let Some(expr) = &arg.expr {
                            self.resolve_expr(expr);
                        }
                    }
                }
            }
            ExprKind::ArrayLiteral { elems } | ExprKind::TypedArrayLiteral { elems, .. } => {
                match elems {
                    nia_ast::ArrayElements::List(elems) => {
                        for elem in elems {
                            self.resolve_expr(elem);
                        }
                    }
                    nia_ast::ArrayElements::Repeat { value, count } => {
                        self.resolve_expr(value);
                        self.resolve_expr(count);
                    }
                }
            }
            ExprKind::StructLiteral { fields } | ExprKind::TypedStructLiteral { fields, .. } => {
                for field in fields {
                    self.resolve_expr(&field.value);
                }
            }
            ExprKind::Unary { expr, .. }
            | ExprKind::OptionalSome { expr }
            | ExprKind::ErrorOk { expr }
            | ExprKind::ErrorErr { expr }
            | ExprKind::Try { expr } => self.resolve_expr(expr),
            ExprKind::Binary { lhs, rhs, .. } | ExprKind::Assign { lhs, rhs, .. } => {
                self.resolve_expr(lhs);
                self.resolve_expr(rhs);
            }
            ExprKind::Cast { expr, .. } => self.resolve_expr(expr),
            ExprKind::Call { callee, args } => {
                self.resolve_callee(callee);
                for arg in args {
                    self.resolve_expr(arg);
                }
            }
            ExprKind::Qualified { lhs, .. } => self.resolve_type_qualified_lhs(lhs),
            ExprKind::Field { lhs, .. } => self.resolve_field_lhs(lhs),
            ExprKind::Index { lhs, index } => {
                self.resolve_expr(lhs);
                match index {
                    IndexArg::Expr(index) => self.resolve_expr(index),
                    IndexArg::Range(range) => {
                        if let Some(start) = &range.start {
                            self.resolve_expr(start);
                        }
                        if let Some(end) = &range.end {
                            self.resolve_expr(end);
                        }
                    }
                }
            }
            ExprKind::Range(range) => {
                if let Some(start) = &range.start {
                    self.resolve_expr(start);
                }
                if let Some(end) = &range.end {
                    self.resolve_expr(end);
                }
            }
            ExprKind::Block(block) => self.resolve_block(block),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.resolve_expr(cond);
                self.resolve_block(then_branch);
                if let Some(else_branch) = else_branch {
                    self.resolve_expr(else_branch);
                }
            }
            ExprKind::IfPattern(if_pattern) => {
                self.resolve_expr(&if_pattern.target);
                for arm in &if_pattern.arms {
                    self.push_scope();
                    self.resolve_pattern(
                        &arm.pattern,
                        LocalKind::ConstBinding,
                        "duplicate if pattern binding",
                    );
                    self.resolve_block(&arm.body);
                    self.pop_scope();
                }
                if let Some(else_branch) = &if_pattern.else_branch {
                    self.resolve_expr(else_branch);
                }
            }
            ExprKind::Switch(switch) => {
                self.resolve_expr(&switch.target);
                for arm in &switch.arms {
                    for pattern in &arm.patterns {
                        self.resolve_switch_pattern(pattern);
                    }
                    match &arm.body {
                        SwitchArmBody::Expr(expr) => self.resolve_expr(expr),
                        SwitchArmBody::Stmt(stmt) => self.resolve_stmt(stmt),
                        SwitchArmBody::Block(block) => self.resolve_block(block),
                    }
                }
            }
        }
    }

    fn resolve_switch_pattern(&mut self, pattern: &SwitchPattern) {
        match &pattern.kind {
            SwitchPatternKind::Wildcard => {}
            SwitchPatternKind::Expr(expr) => self.resolve_expr(expr),
            SwitchPatternKind::Range { start, end, .. } => {
                self.resolve_expr(start);
                self.resolve_expr(end);
            }
        }
    }

    fn resolve_pattern(
        &mut self,
        pattern: &Pattern,
        binding_kind: LocalKind,
        duplicate: &'static str,
    ) {
        self.resolve_pattern_with_span(pattern, binding_kind, pattern.span, duplicate);
    }

    fn resolve_pattern_with_span(
        &mut self,
        pattern: &Pattern,
        binding_kind: LocalKind,
        binding_span: Span,
        duplicate: &'static str,
    ) {
        match &pattern.kind {
            PatternKind::Wildcard | PatternKind::OptionalNull => {}
            PatternKind::Bind {
                name,
                node_key,
                is_mutable,
            } => {
                let kind = if matches!(binding_kind, LocalKind::ComptimeBinding) {
                    LocalKind::ComptimeBinding
                } else if *is_mutable || matches!(binding_kind, LocalKind::Binding) {
                    LocalKind::Binding
                } else {
                    LocalKind::ConstBinding
                };
                self.define(name, kind, binding_span, node_key.clone(), duplicate);
            }
            PatternKind::Pointer(pattern) | PatternKind::MutPointer(pattern) => {
                self.resolve_pattern_with_span(pattern, binding_kind, binding_span, duplicate);
            }
            PatternKind::OptionalSome(pattern)
            | PatternKind::ErrorOk(pattern)
            | PatternKind::ErrorErr(pattern) => {
                self.resolve_pattern_with_span(pattern, binding_kind, binding_span, duplicate);
            }
            PatternKind::Expr(pattern) => self.resolve_expr(pattern),
            PatternKind::Range { start, end, .. } => {
                self.resolve_expr(start);
                self.resolve_expr(end);
            }
        }
    }

    fn resolve_callee(&mut self, callee: &Expr) {
        if let ExprKind::BracketSuffix { callee, args } = &callee.kind {
            self.resolve_callee(callee);
            if self.should_resolve_callee_bracket_args(callee, args) {
                for arg in args {
                    if let Some(expr) = &arg.expr {
                        self.resolve_expr(expr);
                    }
                }
            }
            return;
        }
        self.resolve_expr(callee);
    }

    fn resolve_type_qualified_lhs(&mut self, lhs: &Expr) {
        if !self.try_resolve_type_prefix(lhs) {
            self.resolve_expr(lhs);
        }
    }

    fn resolve_field_lhs(&mut self, lhs: &Expr) {
        self.resolve_expr(lhs);
    }

    fn try_resolve_type_prefix(&mut self, expr: &Expr) -> bool {
        if let ExprKind::BracketSuffix { callee, .. } = &expr.kind {
            return self.try_resolve_type_prefix(callee);
        }
        if matches!(expr.kind, ExprKind::TypeTarget { .. }) {
            self.record_use(expr.node_key.clone(), LocalUse::TypePrefix);
            return true;
        }
        if let ExprKind::Qualified { lhs, .. } = &expr.kind {
            if self
                .values
                .node_qualified_type_prefixes
                .contains_key(&expr.node_key)
            {
                // The Qualified's own span resolves to a type — recurse into
                // lhs so the module-alias span still gets marked, then mark us.
                self.resolve_expr(lhs);
                self.record_use(expr.node_key.clone(), LocalUse::TypePrefix);
                return true;
            }
            return false;
        }
        let ExprKind::Ident(name) = &expr.kind else {
            return false;
        };
        if matches!(
            self.values.node_names.get(&expr.node_key),
            None | Some(ValueNameResolution::LocalDeferred | ValueNameResolution::External(_))
        ) && self.lookup_any(name).is_none()
            && (self.defs.module_scope.types.get(name).is_some()
                || self
                    .values
                    .node_qualified_type_prefixes
                    .contains_key(&expr.node_key))
        {
            self.record_use(expr.node_key.clone(), LocalUse::TypePrefix);
            return true;
        }
        false
    }

    fn should_resolve_expr_bracket_args(
        &self,
        callee: &Expr,
        args: &[nia_ast::BracketArg],
    ) -> bool {
        self.bracket_suffix_can_be_index(args) || !self.bracket_suffix_can_be_generic(callee)
    }

    fn should_resolve_callee_bracket_args(
        &self,
        callee: &Expr,
        args: &[nia_ast::BracketArg],
    ) -> bool {
        self.bracket_suffix_is_unambiguous_index(args)
            || (self.bracket_suffix_can_be_index(args) && self.callee_is_indexable_expr(callee))
            || !self.bracket_suffix_can_be_generic(callee)
    }

    fn bracket_suffix_can_be_generic(&self, callee: &Expr) -> bool {
        match &callee.kind {
            ExprKind::Ident(name) => {
                matches!(
                    self.values.node_names.get(&callee.node_key),
                    Some(ValueNameResolution::Def(_))
                ) || (self.lookup_any(name).is_none()
                    && (self.defs.module_scope.types.get(name).is_some()
                        || self
                            .values
                            .node_qualified_type_prefixes
                            .contains_key(&callee.node_key)))
            }
            ExprKind::Qualified { .. } => {
                self.values
                    .node_qualified_values
                    .contains_key(&callee.node_key)
                    || self
                        .values
                        .node_qualified_type_prefixes
                        .contains_key(&callee.node_key)
            }
            ExprKind::TypeTarget { .. } => true,
            ExprKind::Field { .. } => true,
            ExprKind::BracketSuffix { callee, .. } => self.bracket_suffix_can_be_generic(callee),
            _ => false,
        }
    }

    fn bracket_suffix_can_be_index(&self, args: &[nia_ast::BracketArg]) -> bool {
        let [
            nia_ast::BracketArg {
                expr: Some(expr),
                ty,
                ..
            },
        ] = args
        else {
            return false;
        };
        ty.is_none() || self.expr_is_known_local(expr)
    }

    fn bracket_suffix_is_unambiguous_index(&self, args: &[nia_ast::BracketArg]) -> bool {
        matches!(
            args,
            [nia_ast::BracketArg {
                expr: Some(_),
                ty: None,
                ..
            },]
        )
    }

    fn expr_is_known_local(&self, expr: &Expr) -> bool {
        let ExprKind::Ident(name) = &expr.kind else {
            return false;
        };
        self.lookup_local(name).is_some() || self.lookup_static(name).is_some()
    }

    fn callee_is_indexable_expr(&self, callee: &Expr) -> bool {
        matches!(
            callee.kind,
            ExprKind::Field { .. } | ExprKind::Index { .. } | ExprKind::BracketSuffix { .. }
        )
    }

    fn resolve_ident(&mut self, name: &str, node_key: VersionedNodeKey) {
        if let Some(local) = self.lookup_local(name) {
            self.record_use(node_key, LocalUse::Local(local.id));
            return;
        }
        if let Some(item) = self.lookup_static(name) {
            self.record_use(node_key, LocalUse::Static(item.id));
            return;
        }
        match self.values.node_names.get(&node_key).copied() {
            Some(ValueNameResolution::Def(_)) | Some(ValueNameResolution::External(_)) => {
                self.record_use(node_key, LocalUse::ModuleValue);
            }
            Some(ValueNameResolution::Module) => {
                self.record_use(node_key, LocalUse::Module);
            }
            Some(ValueNameResolution::LocalDeferred) | None => {
                self.record_use(node_key, LocalUse::Unresolved);
            }
            Some(ValueNameResolution::Error) => {
                self.record_use(node_key, LocalUse::ModuleValue);
            }
        }
    }

    fn define(
        &mut self,
        name: &str,
        kind: LocalKind,
        span: Span,
        node_key: VersionedNodeKey,
        duplicate_message: &'static str,
    ) {
        let id = if let Some(definition_ids) = &self.definition_ids {
            let Some(id) = definition_ids.get(&node_key).copied() else {
                self.diagnostics.push(
                    Diagnostic::internal_error(
                        codes::LOCAL_RESOLVER_SCOPE,
                        "local resolver filtered definition has no preallocated id",
                    )
                    .primary(
                        span,
                        "local definition was not present in preallocated local ids",
                    )
                    .debug("name", name)
                    .debug("kind", kind)
                    .debug("node_key", node_key.clone())
                    .finish(),
                );
                return;
            };
            id
        } else {
            self.locals.push(Local {
                name: name.to_string(),
                kind,
                span,
            })
        };
        let debug_node_key = node_key.clone();
        self.node_local_defs.insert(node_key, id);
        let Some(scope) = self.scopes.last_mut() else {
            self.diagnostics.push(
                Diagnostic::internal_error(
                    codes::LOCAL_RESOLVER_SCOPE,
                    "local resolver has no active scope",
                )
                .primary(
                    span,
                    "local definition reached resolver without an active scope",
                )
                .debug("name", name)
                .debug("kind", kind)
                .debug("node_key", debug_node_key)
                .finish(),
            );
            return;
        };
        if let Some(existing) = scope.locals.get(name) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::LOCAL_RESOLUTION,
                span,
                format!("{duplicate_message}: `{name}`"),
            ));
            let _ = existing.span;
            return;
        }
        if let Some(existing) = scope.statics.get(name) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::LOCAL_RESOLUTION,
                span,
                format!("{duplicate_message}: `{name}`"),
            ));
            let _ = existing.span;
            return;
        }
        scope
            .locals
            .insert(name.to_string(), ScopedLocal { id, span });
    }

    fn define_static(
        &mut self,
        name: &str,
        id: nia_ids::GlobalDefId,
        span: Span,
        duplicate_message: &'static str,
    ) {
        let Some(scope) = self.scopes.last_mut() else {
            self.diagnostics.push(Diagnostic::internal_error_at(
                codes::LOCAL_RESOLVER_SCOPE,
                span,
                "local static definition reached resolver without an active scope",
            ));
            return;
        };
        if let Some(existing) = scope.locals.get(name) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::LOCAL_RESOLUTION,
                span,
                format!("{duplicate_message}: `{name}`"),
            ));
            let _ = existing.span;
            return;
        }
        if let Some(existing) = scope.statics.get(name) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::LOCAL_RESOLUTION,
                span,
                format!("{duplicate_message}: `{name}`"),
            ));
            let _ = existing.span;
            return;
        }
        scope
            .statics
            .insert(name.to_string(), ScopedStatic { id, span });
    }

    fn record_use(&mut self, node_key: VersionedNodeKey, use_kind: LocalUse) {
        self.node_uses.insert(node_key, use_kind);
    }

    fn lookup_local(&self, name: &str) -> Option<ScopedLocal> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.locals.get(name).copied())
    }

    fn lookup_static(&self, name: &str) -> Option<ScopedStatic> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.statics.get(name).copied())
    }

    fn lookup_any(&self, name: &str) -> Option<()> {
        (self.lookup_local(name).is_some() || self.lookup_static(name).is_some()).then_some(())
    }

    fn push_scope(&mut self) {
        self.scopes.push(Scope::default());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{ModuleId, collect_module_defs, collect_module_defs_from_active_item_tree};
    use nia_ids::{DefId, GlobalDefId};
    use nia_item_tree::ModuleItemTree;
    use nia_node_id::{NodePosition, SyntaxKind};
    use nia_parser::{parse_module, parse_module_syntax_with_origins};
    use nia_source::{SourceId, SourceRevision, SourceVersion};
    use nia_value_resolve::{
        ProgramDefsContext as ValueProgramDefsContext, resolve_module_values,
        resolve_module_values_from_active_item_tree,
    };

    #[test]
    fn resolves_params_and_local_bindings() {
        let (module, errors) = parse_module(
            r#"
static mut global = 1;

fn add(a: i32, b: i32) i32 {
    let mut sum = a + b + global;
    sum
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let values = resolve_module_values(&module, &defs);
        let locals = resolve_module_locals(&module, &defs, &values);
        assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
        assert!(
            locals
                .node_uses
                .values()
                .any(|use_kind| matches!(use_kind, LocalUse::Local(_)))
        );
        assert!(
            locals
                .node_uses
                .values()
                .any(|use_kind| matches!(use_kind, LocalUse::ModuleValue))
        );
    }

    #[test]
    fn lexical_locals_shadow_module_values() {
        let source = r#"
static mut value = 1;

fn id(value: i32) i32 {
    value
}
"#;
        let (module, errors) = parse_module(source);
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let values = resolve_module_values(&module, &defs);
        let locals = resolve_module_locals(&module, &defs, &values);
        assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
        assert!(
            locals
                .node_uses
                .values()
                .any(|use_kind| matches!(*use_kind, LocalUse::Local(_)))
        );
    }

    #[test]
    fn if_pattern_payload_locals_shadow_external_values_in_field_lhs() {
        let source = r#"
struct S {
    start: i32,
}

fn value(input: ?S) ?i32 {
    if ?range = input {
        ?range.start
    } or null {
        null
    }
}
"#;
        let (module, errors) = parse_module(source);
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let mut values = resolve_module_values(&module, &defs);
        for item in &module.items {
            if let nia_ast::ItemKind::Function(function) = &item.kind
                && function.name == "value"
                && let Some(body) = &function.body
                && let Some(expr) = &body.tail
                && let ExprKind::IfPattern(if_pattern) = &expr.kind
                && let Some(arm) = if_pattern.arms.first()
                && let Some(arm_expr) = arm.body.tail.as_deref()
                && let ExprKind::OptionalSome { expr: some_expr } = &arm_expr.kind
                && let ExprKind::Field { lhs, .. } = &some_expr.kind
                && let ExprKind::Ident(name) = &lhs.kind
                && name == "range"
            {
                values.node_names.insert(
                    lhs.node_key.clone(),
                    ValueNameResolution::External(GlobalDefId {
                        module_id: ModuleId(99),
                        def_id: DefId(1),
                    }),
                );
            }
        }
        let locals = resolve_module_locals(&module, &defs, &values);
        assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
        let range_id = locals
            .locals
            .iter()
            .find_map(|(id, local)| (local.name == "range").then_some(id))
            .expect("expected if pattern payload local");
        assert!(
            locals
                .node_uses
                .values()
                .any(|use_kind| *use_kind == LocalUse::Local(range_id)),
            "{:?}",
            locals.node_uses
        );
    }

    #[test]
    fn records_local_facts_by_source_versioned_node_keys() {
        let version = SourceVersion {
            id: SourceId(4),
            revision: SourceRevision(2),
        };
        let syntax = nia_syntax::parse_source(
            r#"
fn main(a: i32) i32 {
    let mut x = a;
    x
}
"#,
            Some(version),
        );
        let (module, errors, origins) = parse_module_syntax_with_origins(&syntax);
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let values = resolve_module_values(&module, &defs);
        let locals =
            resolve_module_locals_with_origins(&module, &defs, &values, Some(version), &origins);

        assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
        assert!(!locals.node_local_defs.is_empty());
        assert!(!locals.node_uses.is_empty());
        assert!(locals.node_uses.iter().any(|(key, use_kind)| {
            key.source_version() == version
                && key.kind() == SyntaxKind::Expr
                && matches!(key.position(), NodePosition::ChildPathRange { .. })
                && matches!(use_kind, LocalUse::Local(_))
        }));
    }

    #[test]
    fn records_local_facts_by_red_child_path_origins() {
        let version = SourceVersion {
            id: SourceId(5),
            revision: SourceRevision(1),
        };
        let syntax = nia_syntax::parse_source(
            r#"
fn main(a: i32) i32 {
    let mut x = a;
    x
}
"#,
            Some(version),
        );
        let (module, errors, origins) = parse_module_syntax_with_origins(&syntax);
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let values = resolve_module_values(&module, &defs);
        let locals =
            resolve_module_locals_with_origins(&module, &defs, &values, Some(version), &origins);

        assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
        assert!(locals.node_uses.iter().any(|(key, use_kind)| {
            key.source_version() == version
                && key.kind() == SyntaxKind::Expr
                && matches!(key.position(), NodePosition::ChildPathRange { .. })
                && matches!(use_kind, LocalUse::Local(_))
        }));
    }

    #[test]
    fn reports_unresolved_deferred_names() {
        let (module, errors) = parse_module(
            r#"
fn main() i32 {
    missing
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let values = resolve_module_values(&module, &defs);
        let locals = resolve_module_locals(&module, &defs, &values);
        assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
        assert!(
            locals
                .node_uses
                .values()
                .any(|use_kind| matches!(use_kind, LocalUse::Unresolved)),
            "{:?}",
            locals.node_uses
        );
    }

    #[test]
    fn reports_duplicates_in_same_scope() {
        let (module, errors) = parse_module(
            r#"
fn main(a: i32, a: i32) i32 {
    let mut x = 1;
    let mut x = 2;
    x
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let values = resolve_module_values(&module, &defs);
        let locals = resolve_module_locals(&module, &defs, &values);
        assert_eq!(locals.diagnostics.len(), 2);
        assert!(
            locals
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("duplicate parameter name"))
        );
        assert!(
            locals
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("duplicate local binding"))
        );
    }

    #[test]
    fn marks_type_prefixes_for_associated_functions_and_enum_variants() {
        let (module, errors) = parse_module(
            r#"
struct Point {
    x: i32,
}

extend Point {
    fn origin() Point {
        { x: 0 }
    }
}

enum Color {
    Red,
}

fn main() Point {
    let mut c = Color::Red;
    Point::origin()
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let values = resolve_module_values(&module, &defs);
        let locals = resolve_module_locals(&module, &defs, &values);
        assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
        assert!(
            locals
                .node_uses
                .values()
                .any(|use_kind| matches!(use_kind, LocalUse::TypePrefix))
        );
    }

    #[test]
    fn resolves_index_expr_inside_field_bracket_suffix() {
        let (module, errors) = parse_module(
            r#"
struct S {
    x: i32,
}

struct T {
    xs: [4]S,
}

fn main() i32 {
    let mut t: T = { xs: [{ x: 0 }; 4] };
    for i in 0u16..4u16 {
        t.xs[i as usize] = { x: i as i32 };
    }
    t.xs[2].x
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let values = resolve_module_values(&module, &defs);
        let locals = resolve_module_locals(&module, &defs, &values);
        assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
        let i_id = locals
            .locals
            .iter()
            .find_map(|(id, local)| (local.name == "i").then_some(id))
            .expect("expected loop local");
        assert!(
            locals
                .node_uses
                .values()
                .any(|use_kind| *use_kind == LocalUse::Local(i_id)),
            "{:?}",
            locals.node_uses
        );
    }

    #[test]
    fn resolves_local_named_like_type_inside_field_bracket_suffix() {
        let (module, errors) = parse_module(
            r#"
struct S {
    x: i32,
}

struct T {
    xs: [4]S,
}

fn main() i32 {
    let mut t: T = { xs: [{ x: 0 }; 4] };
    let mut i32: usize = 2;
    t.xs[i32].x
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let values = resolve_module_values(&module, &defs);
        let locals = resolve_module_locals(&module, &defs, &values);
        assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
        let i32_id = locals
            .locals
            .iter()
            .find_map(|(id, local)| (local.name == "i32").then_some(id))
            .expect("expected local named i32");
        assert!(
            locals
                .node_uses
                .values()
                .any(|use_kind| *use_kind == LocalUse::Local(i32_id)),
            "{:?}",
            locals.node_uses
        );
    }

    #[test]
    fn resolves_locals_from_active_item_tree_only() {
        let (module, errors) = parse_module(
            r#"
@[if false]
fn skipped() i32 {
    missing
}
@[if true]
fn selected() i32 {
    let mut value = 1;
    value
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let tree = ModuleItemTree::from_module(&module);
        let active = tree.active_items(&mut BoolResolver(false)).unwrap();
        let defs = collect_module_defs_from_active_item_tree(ModuleId(0), &active);
        assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
        let values = resolve_module_values_from_active_item_tree(
            &active,
            &defs,
            ValueProgramDefsContext::empty(),
            &nia_defs::PublicSurfaces::default(),
            &nia_defs::ModuleUsingScope::default(),
        );
        assert!(values.diagnostics.is_empty(), "{:?}", values.diagnostics);
        let locals = resolve_module_locals_from_active_item_tree_with_origins(
            &active,
            &defs,
            &values,
            None,
            &nia_node_id::NodeOriginTable::default(),
        );
        assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
        assert!(locals.locals.iter().any(|(_, local)| local.name == "value"));
    }

    #[test]
    fn filtered_local_resolution_preserves_full_tree_local_ids() {
        let (module, errors) = parse_module(
            r#"
fn unused(a: i32) i32 {
    let mut x = a;
    x
}

fn used(b: i32) i32 {
    let mut y = b;
    y
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let tree = ModuleItemTree::from_module(&module);
        let full = tree.active_items(&mut BoolResolver(true)).unwrap();
        let defs = collect_module_defs_from_active_item_tree(ModuleId(0), &full);
        assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
        let values = resolve_module_values_from_active_item_tree(
            &full,
            &defs,
            ValueProgramDefsContext::empty(),
            &nia_defs::PublicSurfaces::default(),
            &nia_defs::ModuleUsingScope::default(),
        );
        assert!(values.diagnostics.is_empty(), "{:?}", values.diagnostics);
        let full_locals = resolve_module_locals_from_active_item_tree_with_origins(
            &full,
            &defs,
            &values,
            None,
            &nia_node_id::NodeOriginTable::default(),
        );

        let mut filtered = full.clone();
        for item in &mut filtered.items {
            if let ItemTreeNodeKind::Function(function) = &mut item.kind
                && function.name == "unused"
            {
                function.body = None;
            }
        }
        let filtered_values = resolve_module_values_from_active_item_tree(
            &filtered,
            &defs,
            ValueProgramDefsContext::empty(),
            &nia_defs::PublicSurfaces::default(),
            &nia_defs::ModuleUsingScope::default(),
        );
        let filtered_locals = resolve_module_locals_from_filtered_active_item_tree_with_origins(
            &filtered,
            &full,
            &defs,
            &filtered_values,
            None,
            &nia_node_id::NodeOriginTable::default(),
        );
        assert!(
            filtered_locals.diagnostics.is_empty(),
            "{:?}",
            filtered_locals.diagnostics
        );

        for name in ["b", "y"] {
            let full_id = local_id_by_name(&full_locals, name);
            let filtered_id = local_id_by_name(&filtered_locals, name);
            assert_eq!(filtered_id, full_id, "local id changed for {name}");
        }
        let unused_x = local_id_by_name(&full_locals, "x");
        assert!(
            !filtered_locals
                .node_uses
                .values()
                .any(|use_kind| *use_kind == LocalUse::Local(unused_x)),
            "{:?}",
            filtered_locals.node_uses
        );
    }

    fn local_id_by_name(locals: &LocalResolution, name: &str) -> LocalId {
        locals
            .locals
            .iter()
            .find_map(|(id, local)| (local.name == name).then_some(id))
            .unwrap_or_else(|| panic!("expected local `{name}`"))
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
