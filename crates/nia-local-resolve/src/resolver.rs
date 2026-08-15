//! Lexical scope construction and local-use resolution.

use super::*;

mod bracket_resolution;

pub(super) fn resolve_module_locals_from_filtered_items(
    filtered_items: &[ItemTreeNode],
    full_items: &[ItemTreeNode],
    defs: &DefCollection,
    values: &ValueResolution,
    symbols: Option<&dyn SymbolText>,
    node_store: &NodeStore,
) -> LocalResolution {
    let allocated = LocalDefinitionAllocator::allocate_items(full_items);
    let mut resolver = LocalResolver {
        defs,
        values,
        locals: allocated.locals,
        node_local_defs: allocated.node_local_defs.clone(),
        node_uses: HashMap::new(),
        node_type_prefixes: HashMap::new(),
        diagnostics: Vec::new(),
        symbols,
        scopes: Vec::new(),
        self_locals: Vec::new(),
        definition_ids: Some(allocated.node_local_defs),
    };
    resolver.resolve_items(filtered_items);
    finish_local_resolution(resolver, node_store)
}

pub(super) fn resolve_module_locals_from_items(
    items: &[ItemTreeNode],
    defs: &DefCollection,
    values: &ValueResolution,
    node_store: &NodeStore,
) -> LocalResolution {
    resolve_module_locals_from_items_with_symbols(items, defs, values, None, node_store)
}

pub(super) fn resolve_module_locals_from_items_with_symbols(
    items: &[ItemTreeNode],
    defs: &DefCollection,
    values: &ValueResolution,
    symbols: Option<&dyn SymbolText>,
    node_store: &NodeStore,
) -> LocalResolution {
    let mut resolver = LocalResolver {
        defs,
        values,
        locals: LocalMap::default(),
        node_local_defs: HashMap::new(),
        node_uses: HashMap::new(),
        node_type_prefixes: HashMap::new(),
        diagnostics: Vec::new(),
        symbols,
        scopes: Vec::new(),
        self_locals: Vec::new(),
        definition_ids: None,
    };
    resolver.resolve_items(items);
    finish_local_resolution(resolver, node_store)
}

fn finish_local_resolution(resolver: LocalResolver<'_>, node_store: &NodeStore) -> LocalResolution {
    let mut node_local_defs = NodeMap::builder(node_store);
    node_local_defs.extend(resolver.node_local_defs);
    let mut node_uses = NodeMap::builder(node_store);
    node_uses.extend(resolver.node_uses);
    let mut node_type_prefixes = NodeMap::builder(node_store);
    node_type_prefixes.extend(resolver.node_type_prefixes);
    LocalResolution {
        locals: resolver.locals,
        node_local_defs: node_local_defs.finish(),
        node_uses: node_uses.finish(),
        node_type_prefixes: node_type_prefixes.finish(),
        diagnostics: resolver.diagnostics,
    }
}

struct LocalResolver<'a> {
    defs: &'a DefCollection,
    values: &'a ValueResolution,
    locals: LocalMap,
    node_local_defs: HashMap<VersionedNodeKey, LocalId>,
    node_uses: HashMap<VersionedNodeKey, LocalUse>,
    node_type_prefixes: HashMap<VersionedNodeKey, nia_ids::GlobalDefId>,
    diagnostics: Vec<Diagnostic>,
    symbols: Option<&'a dyn SymbolText>,
    scopes: Vec<Scope>,
    self_locals: Vec<Option<ScopedLocal>>,
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
    locals: SymbolMap<ScopedLocal>,
    statics: SymbolMap<ScopedStatic>,
}

impl<'a> LocalResolver<'a> {
    fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_from_optional_resolver(self.symbols, symbol)
    }

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
        let self_stack_len = self.self_locals.len();
        for param in &function.params {
            if let Some(ty) = &param.ty {
                self.resolve_type(ty);
            }
            if param.receiver.is_some() {
                let local = self.define_receiver(param.span, param.node_key.clone());
                self.self_locals.push(local);
            } else if let Some(name) = &param.name {
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
        // Receiver identities are stack-scoped just like lexical scopes. A
        // nested closure or method must not leak its `self` binding into the
        // enclosing function after this callback returns.
        self.self_locals.truncate(self_stack_len);
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
                    LocalKind::ImmutableBinding,
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
        let default_kind = if binding.is_const() {
            LocalKind::ConstBinding
        } else if binding.is_mutable() {
            LocalKind::MutableBinding
        } else {
            LocalKind::ImmutableBinding
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
                .debug("name", binding.name)
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
            | TypeKind::Opaque
            | TypeKind::Never
            | TypeKind::Infer => {}
            TypeKind::Path { segments } => {
                for segment in segments {
                    for arg in &segment.args {
                        match arg {
                            TypeArg::Type(ty) | TypeArg::AssocBinding { ty, .. } => {
                                self.resolve_type(ty);
                            }
                            TypeArg::Const(expr) => self.resolve_expr(expr),
                            TypeArg::TypeOrConst { ty, .. } => self.resolve_type_candidate(ty),
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
            TypeKind::Tuple { elems } => {
                for elem in elems {
                    self.resolve_type(elem);
                }
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
            }
            | TypeKind::Callable {
                params,
                return_type,
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

    fn resolve_type_candidate(&mut self, ty: &TypeRef) {
        if let TypeKind::Path { segments } = &ty.kind {
            for segment in segments {
                for arg in &segment.args {
                    match arg {
                        TypeArg::Type(ty) | TypeArg::AssocBinding { ty, .. } => {
                            self.resolve_type(ty);
                        }
                        TypeArg::Const(expr) => self.resolve_expr(expr),
                        TypeArg::TypeOrConst { ty, .. } => self.resolve_type_candidate(ty),
                    }
                }
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
            ExprKind::SelfValue => {
                if let Some(Some(local)) = self.self_locals.last().copied() {
                    self.record_use(expr.node_key.clone(), LocalUse::Local(local.id));
                } else {
                    self.record_use(expr.node_key.clone(), LocalUse::Unresolved);
                }
            }
            ExprKind::TypeTarget { .. }
            | ExprKind::TraitTarget { .. }
            | ExprKind::PathRoot(_)
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
            ExprKind::Tuple(elems) => {
                for elem in elems {
                    self.resolve_expr(elem);
                }
            }
            ExprKind::Closure {
                captures,
                params,
                body,
            } => {
                for capture in captures {
                    self.resolve_expr(&capture.value);
                }
                self.push_scope();
                for capture in captures {
                    self.define(
                        &capture.name,
                        LocalKind::ImmutableBinding,
                        capture.span,
                        capture.node_key.clone(),
                        "duplicate closure capture",
                    );
                }
                let self_stack_len = self.self_locals.len();
                for param in params {
                    if let Some(ty) = &param.ty {
                        self.resolve_type(ty);
                    }
                    if param.receiver.is_some() {
                        let local = self.define_receiver(param.span, param.node_key.clone());
                        self.self_locals.push(local);
                    } else if let Some(name) = &param.name {
                        self.define(
                            name,
                            LocalKind::Param,
                            param.span,
                            param.node_key.clone(),
                            "duplicate closure parameter name",
                        );
                    }
                }
                self.resolve_expr(body);
                self.self_locals.truncate(self_stack_len);
                self.pop_scope();
            }
            ExprKind::ArrayLiteral { elems } => match elems {
                nia_ast::ArrayElements::List(elems) => {
                    for elem in elems {
                        self.resolve_expr(elem);
                    }
                }
                nia_ast::ArrayElements::Repeat { value, count } => {
                    self.resolve_expr(value);
                    self.resolve_expr(count);
                }
            },
            ExprKind::TypedStructLiteral { fields, .. } => {
                for field in fields {
                    self.resolve_expr(&field.value);
                }
            }
            ExprKind::QualifiedStructLiteral { target, fields } => {
                self.resolve_expr(target);
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
            ExprKind::TupleField { lhs, .. } => self.resolve_expr(lhs),
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
                self.push_scope();
                self.resolve_pattern(
                    &if_pattern.pattern,
                    LocalKind::ImmutableBinding,
                    "duplicate if pattern binding",
                );
                self.resolve_block(&if_pattern.then_branch);
                self.pop_scope();
                if let Some(else_branch) = &if_pattern.else_branch {
                    self.resolve_expr(else_branch);
                }
            }
            ExprKind::Switch(switch) => {
                self.resolve_expr(&switch.target);
                for arm in &switch.arms {
                    self.push_scope();
                    for pattern in &arm.patterns {
                        self.resolve_pattern(
                            pattern,
                            LocalKind::ImmutableBinding,
                            "duplicate switch pattern binding",
                        );
                    }
                    match &arm.body {
                        SwitchArmBody::Expr(expr) => self.resolve_expr(expr),
                        SwitchArmBody::Stmt(stmt) => self.resolve_stmt(stmt),
                        SwitchArmBody::Block(block) => self.resolve_block(block),
                    }
                    self.pop_scope();
                }
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
                let kind = if matches!(binding_kind, LocalKind::ConstBinding) {
                    LocalKind::ConstBinding
                } else if *is_mutable || matches!(binding_kind, LocalKind::MutableBinding) {
                    LocalKind::MutableBinding
                } else {
                    LocalKind::ImmutableBinding
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
            PatternKind::Tuple(fields) => {
                for field in fields {
                    self.resolve_pattern_with_span(field, binding_kind, binding_span, duplicate);
                }
            }
            PatternKind::Nominal {
                constructor: variant,
                fields,
            } => {
                if !self.try_resolve_type_prefix(variant) {
                    self.resolve_expr(variant);
                }
                match fields {
                    nia_ast::NominalPatternFields::Tuple(fields) => {
                        for field in fields {
                            self.resolve_pattern_with_span(
                                field,
                                binding_kind,
                                binding_span,
                                duplicate,
                            );
                        }
                    }
                    nia_ast::NominalPatternFields::Named { fields, .. } => {
                        for field in fields {
                            self.resolve_pattern_with_span(
                                &field.pattern,
                                binding_kind,
                                binding_span,
                                duplicate,
                            );
                        }
                    }
                }
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
        if matches!(
            expr.kind,
            ExprKind::TypeTarget { .. } | ExprKind::TraitTarget { .. }
        ) {
            self.record_use(expr.node_key.clone(), LocalUse::TypePrefix);
            return true;
        }
        if let ExprKind::Qualified { lhs, .. } = &expr.kind {
            if let Some(type_id) = self
                .values
                .node_qualified_type_prefixes
                .get(&expr.node_key)
                .copied()
            {
                // The Qualified's own span resolves to a type — recurse into
                // lhs so the module-alias span still gets marked, then mark us.
                self.resolve_expr(lhs);
                self.record_use(expr.node_key.clone(), LocalUse::TypePrefix);
                self.node_type_prefixes
                    .insert(expr.node_key.clone(), type_id);
                return true;
            }
            return false;
        }
        let ExprKind::Ident(name) = &expr.kind else {
            return false;
        };
        let type_id = self
            .defs
            .module_scope
            .types
            .get(name)
            .map(|def_id| nia_ids::GlobalDefId {
                module_id: self.defs.module_id,
                def_id,
            })
            .or_else(|| {
                self.values
                    .node_qualified_type_prefixes
                    .get(&expr.node_key)
                    .copied()
            });
        if matches!(
            self.values.node_names.get(&expr.node_key),
            None | Some(ValueNameResolution::LocalDeferred | ValueNameResolution::External(_))
        ) && self.lookup_any(name).is_none()
            && let Some(type_id) = type_id
        {
            self.record_use(expr.node_key.clone(), LocalUse::TypePrefix);
            self.node_type_prefixes
                .insert(expr.node_key.clone(), type_id);
            return true;
        }
        false
    }

    fn resolve_ident(&mut self, name: &SymbolId, node_key: VersionedNodeKey) {
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
        name: &SymbolId,
        kind: LocalKind,
        span: Span,
        node_key: VersionedNodeKey,
        duplicate_message: &'static str,
    ) -> Option<ScopedLocal> {
        let binding_name = LocalBindingName::named(*name);
        let id = self.local_definition_id(binding_name, kind, span, &node_key)?;
        let debug_node_key = node_key.clone();
        self.node_local_defs.insert(node_key, id);
        let display_name = self.symbol_name(*name);
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
            return None;
        };
        if let Some(existing) = scope.locals.get(name) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::LOCAL_RESOLUTION,
                span,
                format!("{duplicate_message}: `{display_name}`"),
            ));
            let _ = existing.span;
            return None;
        }
        if let Some(existing) = scope.statics.get(name) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::LOCAL_RESOLUTION,
                span,
                format!("{duplicate_message}: `{display_name}`"),
            ));
            let _ = existing.span;
            return None;
        }
        let local = ScopedLocal { id, span };
        scope.locals.insert(*name, local);
        Some(local)
    }

    fn define_receiver(&mut self, span: Span, node_key: VersionedNodeKey) -> Option<ScopedLocal> {
        let name = LocalBindingName::SelfValue;
        let id = self.local_definition_id(name, LocalKind::Param, span, &node_key)?;
        self.node_local_defs.insert(node_key, id);
        Some(ScopedLocal { id, span })
    }

    fn local_definition_id(
        &mut self,
        name: LocalBindingName,
        kind: LocalKind,
        span: Span,
        node_key: &VersionedNodeKey,
    ) -> Option<LocalId> {
        if let Some(definition_ids) = &self.definition_ids {
            let Some(id) = definition_ids.get(node_key).copied() else {
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
                return None;
            };
            Some(id)
        } else {
            Some(self.locals.push(Local { name, kind, span }))
        }
    }

    fn define_static(
        &mut self,
        name: &SymbolId,
        id: nia_ids::GlobalDefId,
        span: Span,
        duplicate_message: &'static str,
    ) {
        let display_name = self.symbol_name(*name);
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
                format!("{duplicate_message}: `{display_name}`"),
            ));
            let _ = existing.span;
            return;
        }
        if let Some(existing) = scope.statics.get(name) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::LOCAL_RESOLUTION,
                span,
                format!("{duplicate_message}: `{display_name}`"),
            ));
            let _ = existing.span;
            return;
        }
        scope.statics.insert(*name, ScopedStatic { id, span });
    }

    fn record_use(&mut self, node_key: VersionedNodeKey, use_kind: LocalUse) {
        self.node_uses.insert(node_key, use_kind);
    }

    fn lookup_local(&self, name: &SymbolId) -> Option<ScopedLocal> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.locals.get(name).copied())
    }

    fn lookup_static(&self, name: &SymbolId) -> Option<ScopedStatic> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.statics.get(name).copied())
    }

    fn lookup_any(&self, name: &SymbolId) -> Option<()> {
        (self.lookup_local(name).is_some() || self.lookup_static(name).is_some()).then_some(())
    }

    fn push_scope(&mut self) {
        self.scopes.push(Scope::default());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }
}
