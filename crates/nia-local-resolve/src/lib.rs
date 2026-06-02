// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{
    ArrayLen, BindingStmt, Block, Expr, ExprKind, FunctionItem, IndexArg, ItemKind, Module, Stmt,
    StmtKind, SwitchArmBody, SwitchPattern, TypeArg, TypeKind, TypeRef,
};
use nia_defs::DefCollection;
use nia_diagnostic::Diagnostic;
pub use nia_ids::LocalId;
use nia_node_id::{NodeKey, NodeOriginTable, SyntaxKind};
use nia_source::SourceVersion;
use nia_span::Span;
use nia_value_resolve::{ValueNameResolution, ValueResolution};

#[derive(Debug, Clone, PartialEq)]
pub struct LocalResolution {
    pub locals: LocalMap,
    /// Span-keyed facts are the compatibility path used by older lowering and
    /// checking passes that receive AST spans directly.
    pub local_defs: HashMap<Span, LocalId>,
    /// Node-keyed facts are the incremental path. When syntax origins are
    /// available they prefer red child paths over raw spans so duplicated
    /// spans and partial reparses do not silently alias unrelated nodes.
    pub node_local_defs: HashMap<NodeKey, LocalId>,
    pub uses: HashMap<Span, LocalUse>,
    pub node_uses: HashMap<NodeKey, LocalUse>,
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
    ModuleValue,
    ImportAlias,
    TypePrefix,
    Unresolved,
}

pub fn resolve_module_locals(
    module: &Module,
    defs: &DefCollection,
    values: &ValueResolution,
) -> LocalResolution {
    resolve_module_locals_with_source(module, defs, values, None)
}

pub fn resolve_module_locals_with_source(
    module: &Module,
    defs: &DefCollection,
    values: &ValueResolution,
    source_version: Option<SourceVersion>,
) -> LocalResolution {
    resolve_module_locals_with_origins(
        module,
        defs,
        values,
        source_version,
        &NodeOriginTable::default(),
    )
}

pub fn resolve_module_locals_with_origins(
    module: &Module,
    defs: &DefCollection,
    values: &ValueResolution,
    source_version: Option<SourceVersion>,
    origins: &NodeOriginTable,
) -> LocalResolution {
    let mut resolver = LocalResolver {
        source_version,
        origins,
        defs,
        values,
        locals: LocalMap::default(),
        local_defs: HashMap::new(),
        node_local_defs: HashMap::new(),
        uses: HashMap::new(),
        node_uses: HashMap::new(),
        diagnostics: Vec::new(),
        scopes: Vec::new(),
    };
    resolver.resolve_module(module);
    LocalResolution {
        locals: resolver.locals,
        local_defs: resolver.local_defs,
        node_local_defs: resolver.node_local_defs,
        uses: resolver.uses,
        node_uses: resolver.node_uses,
        diagnostics: resolver.diagnostics,
    }
}

struct LocalResolver<'a> {
    source_version: Option<SourceVersion>,
    origins: &'a NodeOriginTable,
    defs: &'a DefCollection,
    values: &'a ValueResolution,
    locals: LocalMap,
    local_defs: HashMap<Span, LocalId>,
    node_local_defs: HashMap<NodeKey, LocalId>,
    uses: HashMap<Span, LocalUse>,
    node_uses: HashMap<NodeKey, LocalUse>,
    diagnostics: Vec<Diagnostic>,
    scopes: Vec<HashMap<String, ScopedLocal>>,
}

#[derive(Debug, Clone, Copy)]
struct ScopedLocal {
    id: LocalId,
    span: Span,
}

impl<'a> LocalResolver<'a> {
    fn resolve_module(&mut self, module: &Module) {
        for item in &module.items {
            match &item.kind {
                ItemKind::Function(function) => self.resolve_function(function),
                ItemKind::Trait(item_trait) => {
                    self.resolve_where_clause(&item_trait.where_clause);
                    for method in &item_trait.methods {
                        self.resolve_function(&method.function);
                    }
                }
                ItemKind::Extend(extend) => {
                    self.resolve_type(&extend.target);
                    if let Some(trait_ref) = &extend.trait_ref {
                        self.resolve_type(trait_ref);
                    }
                    self.resolve_where_clause(&extend.where_clause);
                    for method in &extend.methods {
                        self.resolve_function(&method.function);
                    }
                }
                ItemKind::Enum(item_enum) => {
                    for variant in &item_enum.variants {
                        if let Some(value) = &variant.value {
                            self.resolve_expr(value);
                        }
                    }
                }
                ItemKind::Binding(binding) => {
                    if let Some(ty) = &binding.ty {
                        self.resolve_type(ty);
                    }
                    if let Some(value) = &binding.value {
                        self.resolve_expr(value);
                    }
                }
                ItemKind::Import(_)
                | ItemKind::Using(_)
                | ItemKind::Struct(_)
                | ItemKind::Union(_)
                | ItemKind::TypeAlias(_) => {}
            }
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
                    SyntaxKind::Param,
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
                self.resolve_binding(stmt.span, binding);
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
                if let Some(ty) = &for_stmt.binding.ty {
                    self.resolve_type(ty);
                }
                self.define(
                    &for_stmt.binding.name,
                    if for_stmt.binding.is_const {
                        LocalKind::ConstBinding
                    } else {
                        LocalKind::Binding
                    },
                    for_stmt.binding.span,
                    SyntaxKind::Stmt,
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

    fn resolve_binding(&mut self, span: Span, binding: &BindingStmt) {
        if let Some(ty) = &binding.ty {
            self.resolve_type(ty);
        }
        if let Some(value) = &binding.value {
            self.resolve_expr(value);
        }
        self.define(
            &binding.name,
            if binding.is_comptime {
                LocalKind::ComptimeBinding
            } else if binding.is_const {
                LocalKind::ConstBinding
            } else {
                LocalKind::Binding
            },
            span,
            SyntaxKind::Stmt,
            "duplicate local binding",
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
            TypeKind::Pointer { elem, .. } | TypeKind::Slice { elem, .. } => {
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
                self.resolve_ident(name, expr.span);
            }
            ExprKind::Builtin { .. }
            | ExprKind::TypeTarget { .. }
            | ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::ByteString(_)
            | ExprKind::CString(_)
            | ExprKind::Char(_)
            | ExprKind::ByteChar(_)
            | ExprKind::Raw(_)
            | ExprKind::Bool(_)
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
            ExprKind::Unary { expr, .. } => self.resolve_expr(expr),
            ExprKind::Binary { lhs, rhs, .. } | ExprKind::Assign { lhs, rhs, .. } => {
                self.resolve_expr(lhs);
                self.resolve_expr(rhs);
            }
            ExprKind::Cast { expr, .. } => self.resolve_expr(expr),
            ExprKind::Call { callee, args } => {
                self.resolve_callee(callee);
                if let ExprKind::Builtin { name, .. } = &callee.kind
                    && name == "asm"
                {
                    for arg in args {
                        self.resolve_asm_config(arg);
                    }
                } else {
                    for arg in args {
                        self.resolve_expr(arg);
                    }
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
            ExprKind::Switch(switch) => {
                self.resolve_expr(&switch.target);
                for arm in &switch.arms {
                    for pattern in &arm.patterns {
                        match pattern {
                            SwitchPattern::Default => {}
                            SwitchPattern::Expr(pattern) => self.resolve_expr(pattern),
                            SwitchPattern::Range { start, end, .. } => {
                                self.resolve_expr(start);
                                self.resolve_expr(end);
                            }
                        }
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

    fn resolve_asm_config(&mut self, expr: &Expr) {
        let ExprKind::StructLiteral { fields } = &expr.kind else {
            self.resolve_expr(expr);
            return;
        };
        for field in fields {
            match field.name.as_str() {
                "inputs" | "outputs" => self.resolve_expr(&field.value),
                "code" | "clobbers" => {}
                _ => self.resolve_expr(&field.value),
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
            self.record_use(expr.span, LocalUse::TypePrefix);
            return true;
        }
        if let ExprKind::Qualified { lhs, .. } = &expr.kind {
            if self.values.qualified_type_prefixes.contains_key(&expr.span) {
                // The Qualified's own span resolves to a type — recurse into
                // lhs so the import-alias span still gets marked, then mark us.
                self.resolve_expr(lhs);
                self.record_use(expr.span, LocalUse::TypePrefix);
                return true;
            }
            return false;
        }
        let ExprKind::Ident(name) = &expr.kind else {
            return false;
        };
        if matches!(
            self.values.names.get(&expr.span),
            None | Some(ValueNameResolution::LocalDeferred | ValueNameResolution::External(_))
        ) && self.lookup(name).is_none()
            && (self.defs.module_scope.types.get(name).is_some()
                || self.values.qualified_type_prefixes.contains_key(&expr.span))
        {
            self.record_use(expr.span, LocalUse::TypePrefix);
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
                    self.values.names.get(&callee.span),
                    Some(ValueNameResolution::Def(_))
                ) || (self.lookup(name).is_none()
                    && (self.defs.module_scope.types.get(name).is_some()
                        || self
                            .values
                            .qualified_type_prefixes
                            .contains_key(&callee.span)))
            }
            ExprKind::Qualified { .. } => {
                self.values.qualified_values.contains_key(&callee.span)
                    || self
                        .values
                        .qualified_type_prefixes
                        .contains_key(&callee.span)
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
        self.lookup(name).is_some()
    }

    fn callee_is_indexable_expr(&self, callee: &Expr) -> bool {
        matches!(
            callee.kind,
            ExprKind::Field { .. } | ExprKind::Index { .. } | ExprKind::BracketSuffix { .. }
        )
    }

    fn resolve_ident(&mut self, name: &str, span: Span) {
        match self.values.names.get(&span) {
            Some(ValueNameResolution::Def(_)) | Some(ValueNameResolution::External(_)) => {
                self.record_use(span, LocalUse::ModuleValue);
            }
            Some(ValueNameResolution::ImportAlias) => {
                self.record_use(span, LocalUse::ImportAlias);
            }
            Some(ValueNameResolution::LocalDeferred) | None => {
                if let Some(local) = self.lookup(name) {
                    self.record_use(span, LocalUse::Local(local.id));
                } else {
                    self.record_use(span, LocalUse::Unresolved);
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!("unknown local or value `{name}`"),
                    ));
                }
            }
            Some(ValueNameResolution::Error) => {
                self.record_use(span, LocalUse::Unresolved);
            }
        }
    }

    fn define(
        &mut self,
        name: &str,
        kind: LocalKind,
        span: Span,
        syntax_kind: SyntaxKind,
        duplicate_message: &'static str,
    ) {
        let id = self.locals.push(Local {
            name: name.to_string(),
            kind,
            span,
        });
        self.local_defs.insert(span, id);
        if let Some(key) = self.node_key(syntax_kind, span) {
            self.node_local_defs.insert(key, id);
        }
        let Some(scope) = self.scopes.last_mut() else {
            self.diagnostics.push(Diagnostic::error(
                span,
                "internal compiler error: local resolver has no active scope",
            ));
            return;
        };
        if let Some(existing) = scope.get(name) {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("{duplicate_message}: `{name}`"),
            ));
            let _ = existing.span;
            return;
        }
        scope.insert(name.to_string(), ScopedLocal { id, span });
    }

    fn record_use(&mut self, span: Span, use_kind: LocalUse) {
        self.uses.insert(span, use_kind);
        if let Some(key) = self.node_key(SyntaxKind::Expr, span) {
            self.node_uses.insert(key, use_kind);
        }
    }

    fn node_key(&self, kind: SyntaxKind, span: Span) -> Option<NodeKey> {
        // Origin keys are tied to the parsed red-node path. The span fallback
        // keeps non-incremental callers useful, but it is less precise when two
        // recovered AST nodes share a span.
        self.origins.get(kind, span).cloned().or_else(|| {
            self.source_version
                .map(|version| NodeKey::span(version, kind, span))
        })
    }

    fn lookup(&self, name: &str) -> Option<ScopedLocal> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{ModuleId, collect_module_defs};
    use nia_node_id::{NodePosition, SyntaxKind};
    use nia_parser::{parse_module, parse_module_syntax_with_origins};
    use nia_source::{SourceId, SourceRevision, SourceVersion};
    use nia_value_resolve::resolve_module_values;

    #[test]
    fn resolves_params_and_local_bindings() {
        let (module, errors) = parse_module(
            r#"
var global = 1;

fn add(a: i32, b: i32) i32 {
    var sum = a + b + global;
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
                .uses
                .values()
                .any(|use_kind| matches!(use_kind, LocalUse::Local(_)))
        );
        assert!(
            locals
                .uses
                .values()
                .any(|use_kind| matches!(use_kind, LocalUse::ModuleValue))
        );
    }

    #[test]
    fn records_local_facts_by_source_versioned_node_keys() {
        let (module, errors) = parse_module(
            r#"
fn main(a: i32) i32 {
    var x = a;
    x
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let values = resolve_module_values(&module, &defs);
        let version = SourceVersion {
            id: SourceId(4),
            revision: SourceRevision(2),
        };
        let locals = resolve_module_locals_with_source(&module, &defs, &values, Some(version));

        assert!(locals.diagnostics.is_empty(), "{:?}", locals.diagnostics);
        assert!(!locals.node_local_defs.is_empty());
        assert!(!locals.node_uses.is_empty());
        assert!(locals.node_uses.iter().any(|(key, use_kind)| {
            key.source_version() == version
                && key.kind == SyntaxKind::Expr
                && matches!(key.position, NodePosition::Span(_))
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
    var x = a;
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
                && key.kind == SyntaxKind::Expr
                && matches!(key.position, NodePosition::ChildPathRange { .. })
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
        assert_eq!(locals.diagnostics.len(), 1);
        assert!(
            locals.diagnostics[0]
                .message
                .contains("unknown local or value `missing`")
        );
    }

    #[test]
    fn reports_duplicates_in_same_scope() {
        let (module, errors) = parse_module(
            r#"
fn main(a: i32, a: i32) i32 {
    var x = 1;
    var x = 2;
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
                .any(|diagnostic| diagnostic.message.contains("duplicate parameter name"))
        );
        assert!(
            locals
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("duplicate local binding"))
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
    var c = Color::Red;
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
                .uses
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
    var t: T = { xs: [{ x: 0 }; 4] };
    for i: u16 in 0u16..4u16 {
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
                .uses
                .values()
                .any(|use_kind| *use_kind == LocalUse::Local(i_id)),
            "{:?}",
            locals.uses
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
    var t: T = { xs: [{ x: 0 }; 4] };
    var i32: usize = 2;
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
                .uses
                .values()
                .any(|use_kind| *use_kind == LocalUse::Local(i32_id)),
            "{:?}",
            locals.uses
        );
    }
}
