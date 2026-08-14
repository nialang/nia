//! Stable local-id allocation for filtered semantic passes.

use super::*;

/// Preallocates locals by walking the complete active item tree.
///
/// Incremental queries may later resolve only a filtered subset of that tree.
/// Allocating from the complete tree first keeps every surviving binding's
/// `LocalId` independent of which sibling bodies were selected for resolution.
#[derive(Default)]
pub(super) struct LocalDefinitionAllocator {
    pub(super) locals: LocalMap,
    pub(super) node_local_defs: HashMap<VersionedNodeKey, LocalId>,
}

impl LocalDefinitionAllocator {
    pub(super) fn allocate_items(items: &[ItemTreeNode]) -> Self {
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
            if param.receiver.is_some() {
                self.allocate_receiver(param.span, param.node_key.clone());
            } else if let Some(name) = &param.name {
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
                self.allocate_pattern(&for_stmt.pattern, LocalKind::ImmutableBinding);
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
        let default_kind = if binding.is_const() {
            LocalKind::ConstBinding
        } else if binding.is_mutable() {
            LocalKind::MutableBinding
        } else {
            LocalKind::ImmutableBinding
        };
        self.allocate_pattern_with_span(&binding.pattern, default_kind, span);
    }

    fn allocate_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Ident(_)
            | ExprKind::SelfValue
            | ExprKind::PathRoot(_)
            | ExprKind::TypeTarget { .. }
            | ExprKind::TraitTarget { .. }
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
            ExprKind::Tuple(elems) => {
                for elem in elems {
                    self.allocate_expr(elem);
                }
            }
            ExprKind::Closure {
                captures,
                params,
                body,
                ..
            } => {
                for capture in captures {
                    self.allocate_expr(&capture.value);
                    self.allocate_definition(
                        &capture.name,
                        LocalKind::ImmutableBinding,
                        capture.span,
                        capture.node_key.clone(),
                    );
                }
                for param in params {
                    if param.receiver.is_some() {
                        self.allocate_receiver(param.span, param.node_key.clone());
                    } else if let Some(name) = &param.name {
                        self.allocate_definition(
                            name,
                            LocalKind::Param,
                            param.span,
                            param.node_key.clone(),
                        );
                    }
                }
                self.allocate_expr(body);
            }
            ExprKind::ArrayLiteral { elems } | ExprKind::TypedArrayLiteral { elems, .. } => {
                self.allocate_array_elements(elems);
            }
            ExprKind::TypedStructLiteral { fields, .. } => {
                for field in fields {
                    self.allocate_expr(&field.value);
                }
            }
            ExprKind::QualifiedStructLiteral { target, fields } => {
                self.allocate_expr(target);
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
            ExprKind::Qualified { lhs, .. }
            | ExprKind::Field { lhs, .. }
            | ExprKind::TupleField { lhs, .. } => self.allocate_expr(lhs),
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
                self.allocate_pattern(&if_pattern.pattern, LocalKind::ImmutableBinding);
                self.allocate_block(&if_pattern.then_branch);
                if let Some(else_branch) = &if_pattern.else_branch {
                    self.allocate_expr(else_branch);
                }
            }
            ExprKind::Switch(switch) => {
                self.allocate_expr(&switch.target);
                for arm in &switch.arms {
                    for pattern in &arm.patterns {
                        self.allocate_pattern(pattern, LocalKind::ImmutableBinding);
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
                let kind = if matches!(binding_kind, LocalKind::ConstBinding) {
                    LocalKind::ConstBinding
                } else if *is_mutable || matches!(binding_kind, LocalKind::MutableBinding) {
                    LocalKind::MutableBinding
                } else {
                    LocalKind::ImmutableBinding
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
            PatternKind::Tuple(fields) => {
                for field in fields {
                    self.allocate_pattern_with_span(field, binding_kind, binding_span);
                }
            }
            PatternKind::EnumVariant { variant, fields } => {
                self.allocate_expr(variant);
                match fields {
                    nia_ast::EnumVariantPatternFields::Tuple(fields) => {
                        for field in fields {
                            self.allocate_pattern_with_span(field, binding_kind, binding_span);
                        }
                    }
                    nia_ast::EnumVariantPatternFields::Named(fields) => {
                        for field in fields {
                            self.allocate_pattern_with_span(
                                &field.pattern,
                                binding_kind,
                                binding_span,
                            );
                        }
                    }
                }
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
        name: &SymbolId,
        kind: LocalKind,
        span: Span,
        node_key: VersionedNodeKey,
    ) {
        self.allocate_local(LocalBindingName::named(*name), kind, span, node_key);
    }

    fn allocate_receiver(&mut self, span: Span, node_key: VersionedNodeKey) {
        self.allocate_local(
            LocalBindingName::SelfValue,
            LocalKind::Param,
            span,
            node_key,
        );
    }

    fn allocate_local(
        &mut self,
        name: LocalBindingName,
        kind: LocalKind,
        span: Span,
        node_key: VersionedNodeKey,
    ) {
        let id = self.locals.push(Local { name, kind, span });
        self.node_local_defs.insert(node_key, id);
    }
}
