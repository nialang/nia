use std::collections::HashSet;

use crate::{ComptimeModuleInput, ComptimeModuleLowering};
use nia_ast::{Expr, ItemKind};
use nia_comptime_ir::{
    ResolvedComptimeEnum, ResolvedComptimeEnumVariant, ResolvedComptimeExpr,
    ResolvedComptimeLocalInitializer, ResolvedComptimeModule,
};
use nia_defs::{DefId, DefKind};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId, LocalId};
use nia_local_resolve::LocalKind;
use nia_sema_ir::{SemanticUseTable, SemanticValueUse};
use nia_span::Span;

pub fn lower_module_comptime(input: ComptimeModuleInput<'_>) -> ComptimeModuleLowering {
    let mut lowerer = ComptimeModuleLowerer {
        input,
        module: ResolvedComptimeModule::default(),
        diagnostics: Vec::new(),
    };
    lowerer.lower_module();
    ComptimeModuleLowering {
        module: lowerer.module,
        diagnostics: lowerer.diagnostics,
    }
}
struct ComptimeModuleLowerer<'a> {
    input: ComptimeModuleInput<'a>,
    module: ResolvedComptimeModule,
    diagnostics: Vec<Diagnostic>,
}

impl ComptimeModuleLowerer<'_> {
    fn lower_module(&mut self) {
        for item in &self.input.module.items {
            match &item.kind {
                ItemKind::Enum(item_enum) => self.lower_enum(item, item_enum),
                ItemKind::Binding(binding) if binding.is_comptime => {
                    self.lower_global_initializer(item.span, binding)
                }
                ItemKind::Function(function) if function.is_comptime => {
                    self.lower_function(function)
                }
                ItemKind::Extend(extend) => {
                    for associated_value in &extend.associated_values {
                        self.lower_global_initializer(
                            associated_value.span,
                            &associated_value.binding,
                        );
                    }
                    for method in &extend.methods {
                        if method.function.is_comptime {
                            self.lower_function(&method.function);
                        }
                    }
                }
                _ => {}
            }
        }
        for (local_id, local) in self.input.locals.locals.iter() {
            if local.kind == LocalKind::ComptimeBinding
                && let Some((expr, explicit_type)) = self.local_initializer(local_id)
                && let Some(value) = self.lower_expr(&expr)
            {
                self.module.insert_local_initializer(
                    local_id,
                    ResolvedComptimeLocalInitializer::new(explicit_type, value),
                );
            }
        }
        for (id, expr) in self.input.const_exprs {
            if let Some(lowered) = self.lower_expr(expr) {
                self.module.insert_const_expr(*id, lowered);
            }
        }
    }

    fn lower_enum(&mut self, item: &nia_ast::Item, item_enum: &nia_ast::EnumItem) {
        let Some(enum_id) = self.def_id_for_node(&item.node_key, item.span, DefKind::Enum) else {
            return;
        };
        let variants = item_enum
            .variants
            .iter()
            .filter_map(|variant| {
                let variant_id =
                    self.def_id_for_node(&variant.node_key, variant.span, DefKind::EnumVariant)?;
                let value = variant
                    .value
                    .as_ref()
                    .and_then(|expr| self.lower_expr(expr));
                Some(ResolvedComptimeEnumVariant::new(
                    self.global_def_id(variant_id),
                    variant.span,
                    value,
                ))
            })
            .collect();
        self.module.push_enum(ResolvedComptimeEnum::new(
            self.global_def_id(enum_id),
            item.span,
            variants,
        ));
    }

    fn lower_global_initializer(&mut self, item_span: Span, binding: &nia_ast::BindingItem) {
        let Some(def_id) = self.def_id_for_node(&binding.node_key, item_span, DefKind::Comptime)
        else {
            return;
        };
        let value = binding.value.as_ref();
        let Some(value) = value else {
            return;
        };
        if let Some(value) = self.lower_expr(value) {
            self.module
                .insert_global_initializer(self.global_def_id(def_id), value);
        }
    }

    fn lower_function(&mut self, function: &nia_ast::FunctionItem) {
        let Some(def_id) =
            self.def_id_for_node(&function.node_key, function.span, DefKind::Function)
        else {
            return;
        };
        let function_locals = self.function_locals(function);
        let semantic_uses = self.semantic_uses_with_allowed_locals(&function_locals);
        let context = nia_comptime_ir::ResolvedComptimeLowerInputs::new(&semantic_uses);
        match nia_comptime_ir::lower_function_resolved_with_context(
            function.span,
            function,
            &context,
        ) {
            Ok(function) => {
                self.module
                    .insert_function(self.global_def_id(def_id), function);
            }
            Err(err) => {
                self.diagnostics
                    .push(Diagnostic::user_error_at("E0401", err.span, err.message))
            }
        }
    }

    fn lower_expr(&mut self, expr: &Expr) -> Option<ResolvedComptimeExpr> {
        let mut allowed_locals = HashSet::new();
        self.collect_expr_locals(expr, &mut allowed_locals);
        let semantic_uses = self.semantic_uses_with_allowed_locals(&allowed_locals);
        let context = nia_comptime_ir::ResolvedComptimeLowerInputs::new(&semantic_uses);
        match nia_comptime_ir::lower_expr_resolved_with_context(expr, &context) {
            Ok(expr) => Some(expr),
            Err(err) => {
                self.diagnostics
                    .push(Diagnostic::user_error_at("E0401", err.span, err.message));
                None
            }
        }
    }

    fn semantic_uses_with_allowed_locals(
        &self,
        allowed_locals: &HashSet<LocalId>,
    ) -> SemanticUseTable {
        let mut builder = SemanticUseTable::builder();
        for (key, value_use) in &self.input.semantic_uses.node_value_uses {
            match value_use {
                SemanticValueUse::Local(local_id)
                    if allowed_locals.contains(local_id)
                        || self
                            .input
                            .locals
                            .locals
                            .get(*local_id)
                            .is_some_and(|local| local.kind == LocalKind::ComptimeBinding) =>
                {
                    builder.insert_node_local_value_use(key.clone(), *local_id);
                }
                SemanticValueUse::Global(global_id) => {
                    builder.insert_node_global_value_use(key.clone(), *global_id);
                }
                SemanticValueUse::Local(_) => {}
            }
        }
        builder.extend_node_builtin_associated_values(
            self.input
                .semantic_uses
                .node_builtin_associated_values
                .iter()
                .map(|(key, value)| (key.clone(), *value)),
        );
        builder.extend_node_local_defs(
            self.input
                .semantic_uses
                .node_local_defs
                .iter()
                .map(|(key, local_id)| (key.clone(), *local_id)),
        );
        builder.extend_node_type_uses(
            self.input
                .semantic_uses
                .node_type_uses
                .iter()
                .map(|(key, ty)| (key.clone(), *ty)),
        );
        builder.finish()
    }

    fn function_locals(&self, function: &nia_ast::FunctionItem) -> HashSet<LocalId> {
        let mut locals = HashSet::new();
        for param in &function.params {
            if let Some(local_id) = self.input.semantic_uses.node_local_def(&param.node_key) {
                locals.insert(local_id);
            }
        }
        if let Some(body) = &function.body {
            self.collect_block_locals(body, &mut locals);
        }
        locals
    }

    fn collect_block_locals(&self, block: &nia_ast::Block, out: &mut HashSet<LocalId>) {
        for stmt in &block.stmts {
            self.collect_stmt_locals(stmt, out);
        }
        if let Some(tail) = block.tail.as_deref() {
            self.collect_expr_locals(tail, out);
        }
    }

    fn collect_stmt_locals(&self, stmt: &nia_ast::Stmt, out: &mut HashSet<LocalId>) {
        match &stmt.kind {
            nia_ast::StmtKind::Binding(binding) => {
                if let Some(local_id) = self.input.semantic_uses.node_local_def(&stmt.node_key) {
                    out.insert(local_id);
                }
                if let Some(value) = &binding.value {
                    self.collect_expr_locals(value, out);
                }
            }
            nia_ast::StmtKind::Expr(expr)
            | nia_ast::StmtKind::Return(Some(expr))
            | nia_ast::StmtKind::Defer(expr) => self.collect_expr_locals(expr, out),
            nia_ast::StmtKind::ForIn(for_stmt) => {
                if for_stmt.pattern.name().is_some()
                    && let Some(local_id) = self
                        .input
                        .semantic_uses
                        .node_local_def(&for_stmt.pattern.node_key)
                {
                    out.insert(local_id);
                }
                self.collect_expr_locals(&for_stmt.iter, out);
                self.collect_block_locals(&for_stmt.body, out);
            }
            nia_ast::StmtKind::While(while_stmt) => {
                self.collect_expr_locals(&while_stmt.cond, out);
                self.collect_block_locals(&while_stmt.body, out);
            }
            nia_ast::StmtKind::Loop(loop_stmt) => self.collect_block_locals(&loop_stmt.body, out),
            nia_ast::StmtKind::Using(_)
            | nia_ast::StmtKind::Return(None)
            | nia_ast::StmtKind::Break
            | nia_ast::StmtKind::Continue => {}
        }
    }

    fn collect_expr_locals(&self, expr: &nia_ast::Expr, out: &mut HashSet<LocalId>) {
        match &expr.kind {
            nia_ast::ExprKind::BracketSuffix { callee, args } => {
                self.collect_expr_locals(callee, out);
                for arg in args {
                    if let Some(expr) = &arg.expr {
                        self.collect_expr_locals(expr, out);
                    }
                }
            }
            nia_ast::ExprKind::ArrayLiteral { elems }
            | nia_ast::ExprKind::TypedArrayLiteral { elems, .. } => match elems {
                nia_ast::ArrayElements::List(elems) => {
                    for elem in elems {
                        self.collect_expr_locals(elem, out);
                    }
                }
                nia_ast::ArrayElements::Repeat { value, count } => {
                    self.collect_expr_locals(value, out);
                    self.collect_expr_locals(count, out);
                }
            },
            nia_ast::ExprKind::StructLiteral { fields }
            | nia_ast::ExprKind::TypedStructLiteral { fields, .. } => {
                for field in fields {
                    self.collect_expr_locals(&field.value, out);
                }
            }
            nia_ast::ExprKind::Unary { expr, .. }
            | nia_ast::ExprKind::OptionalSome { expr }
            | nia_ast::ExprKind::ErrorOk { expr }
            | nia_ast::ExprKind::ErrorErr { expr }
            | nia_ast::ExprKind::Try { expr }
            | nia_ast::ExprKind::Cast { expr, .. } => self.collect_expr_locals(expr, out),
            nia_ast::ExprKind::Binary { lhs, rhs, .. }
            | nia_ast::ExprKind::Assign { lhs, rhs, .. } => {
                self.collect_expr_locals(lhs, out);
                self.collect_expr_locals(rhs, out);
            }
            nia_ast::ExprKind::Call { callee, args } => {
                self.collect_expr_locals(callee, out);
                for arg in args {
                    self.collect_expr_locals(arg, out);
                }
            }
            nia_ast::ExprKind::Qualified { lhs, .. } | nia_ast::ExprKind::Field { lhs, .. } => {
                self.collect_expr_locals(lhs, out);
            }
            nia_ast::ExprKind::Index { lhs, index } => {
                self.collect_expr_locals(lhs, out);
                match index {
                    nia_ast::IndexArg::Expr(index) => self.collect_expr_locals(index, out),
                    nia_ast::IndexArg::Range(range) => self.collect_range_locals(range, out),
                }
            }
            nia_ast::ExprKind::Ident(_) => {
                if let Some(SemanticValueUse::Local(local_id)) =
                    self.input.semantic_uses.node_value_use(&expr.node_key)
                {
                    out.insert(local_id);
                }
            }
            nia_ast::ExprKind::Range(range) => self.collect_range_locals(range, out),
            nia_ast::ExprKind::Block(block) => self.collect_block_locals(block, out),
            nia_ast::ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.collect_expr_locals(cond, out);
                self.collect_block_locals(then_branch, out);
                if let Some(else_branch) = else_branch.as_deref() {
                    self.collect_expr_locals(else_branch, out);
                }
            }
            nia_ast::ExprKind::IfPattern(if_pattern) => {
                self.collect_expr_locals(&if_pattern.target, out);
                for arm in &if_pattern.arms {
                    self.collect_pattern_locals(&arm.pattern, out);
                    self.collect_block_locals(&arm.body, out);
                }
                if let Some(else_branch) = if_pattern.else_branch.as_deref() {
                    self.collect_expr_locals(else_branch, out);
                }
            }
            nia_ast::ExprKind::ComptimeIf(comptime_if) => {
                self.collect_expr_locals(&comptime_if.cond, out);
                self.collect_block_locals(&comptime_if.then_branch, out);
                if let Some(else_branch) = comptime_if.else_branch.as_deref() {
                    self.collect_expr_locals(else_branch, out);
                }
            }
            nia_ast::ExprKind::Switch(switch) => {
                self.collect_expr_locals(&switch.target, out);
                for arm in &switch.arms {
                    for pattern in &arm.patterns {
                        self.collect_switch_pattern_locals(pattern, out);
                    }
                    match &arm.body {
                        nia_ast::SwitchArmBody::Expr(expr) => self.collect_expr_locals(expr, out),
                        nia_ast::SwitchArmBody::Stmt(stmt) => self.collect_stmt_locals(stmt, out),
                        nia_ast::SwitchArmBody::Block(block) => {
                            self.collect_block_locals(block, out)
                        }
                    }
                }
            }
            nia_ast::ExprKind::Error
            | nia_ast::ExprKind::Integer(_)
            | nia_ast::ExprKind::Float(_)
            | nia_ast::ExprKind::String(_)
            | nia_ast::ExprKind::ByteString(_)
            | nia_ast::ExprKind::Char(_)
            | nia_ast::ExprKind::ByteChar(_)
            | nia_ast::ExprKind::Raw(_)
            | nia_ast::ExprKind::Bool(_)
            | nia_ast::ExprKind::Null
            | nia_ast::ExprKind::Underscore
            | nia_ast::ExprKind::Builtin { .. }
            | nia_ast::ExprKind::TypeTarget { .. } => {}
        }
    }

    fn collect_switch_pattern_locals(
        &self,
        pattern: &nia_ast::SwitchPattern,
        out: &mut HashSet<LocalId>,
    ) {
        match &pattern.kind {
            nia_ast::SwitchPatternKind::Wildcard => {}
            nia_ast::SwitchPatternKind::Expr(expr) => self.collect_expr_locals(expr, out),
            nia_ast::SwitchPatternKind::Range { start, end, .. } => {
                self.collect_expr_locals(start, out);
                self.collect_expr_locals(end, out);
            }
        }
    }

    fn collect_range_locals(&self, range: &nia_ast::SliceRange, out: &mut HashSet<LocalId>) {
        if let Some(start) = range.start.as_deref() {
            self.collect_expr_locals(start, out);
        }
        if let Some(end) = range.end.as_deref() {
            self.collect_expr_locals(end, out);
        }
    }

    fn collect_pattern_locals(&self, pattern: &nia_ast::Pattern, out: &mut HashSet<LocalId>) {
        match &pattern.kind {
            nia_ast::PatternKind::Bind { node_key, .. } => {
                if let Some(local_id) = self.input.semantic_uses.node_local_def(node_key) {
                    out.insert(local_id);
                }
            }
            nia_ast::PatternKind::OptionalSome(pattern)
            | nia_ast::PatternKind::ErrorOk(pattern)
            | nia_ast::PatternKind::ErrorErr(pattern) => self.collect_pattern_locals(pattern, out),
            nia_ast::PatternKind::Expr(expr) => self.collect_expr_locals(expr, out),
            nia_ast::PatternKind::Range { start, end, .. } => {
                self.collect_expr_locals(start, out);
                self.collect_expr_locals(end, out);
            }
            nia_ast::PatternKind::Wildcard | nia_ast::PatternKind::OptionalNull => {}
        }
    }

    fn local_initializer(&self, local_id: LocalId) -> Option<(Expr, Option<InternedTyId>)> {
        self.input
            .module
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Function(function) => function
                    .body
                    .as_ref()
                    .and_then(|body| self.local_initializer_in_block(local_id, body)),
                ItemKind::Extend(extend) => extend.methods.iter().find_map(|method| {
                    method
                        .function
                        .body
                        .as_ref()
                        .and_then(|body| self.local_initializer_in_block(local_id, body))
                }),
                _ => None,
            })
    }

    fn local_initializer_in_block(
        &self,
        local_id: LocalId,
        block: &nia_ast::Block,
    ) -> Option<(Expr, Option<InternedTyId>)> {
        for stmt in &block.stmts {
            match &stmt.kind {
                nia_ast::StmtKind::Binding(binding)
                    if self.input.semantic_uses.node_local_def(&stmt.node_key)
                        == Some(local_id) =>
                {
                    return binding.value.clone().map(|value| {
                        (
                            value,
                            binding.ty.as_ref().and_then(|ty| {
                                self.input.semantic_uses.node_type_use(&ty.node_key)
                            }),
                        )
                    });
                }
                nia_ast::StmtKind::ForIn(for_stmt) => {
                    if let Some(value) = self.local_initializer_in_block(local_id, &for_stmt.body) {
                        return Some(value);
                    }
                }
                nia_ast::StmtKind::While(while_stmt) => {
                    if let Some(value) = self.local_initializer_in_block(local_id, &while_stmt.body)
                    {
                        return Some(value);
                    }
                }
                nia_ast::StmtKind::Loop(loop_stmt) => {
                    if let Some(value) = self.local_initializer_in_block(local_id, &loop_stmt.body)
                    {
                        return Some(value);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn def_id_for_node(
        &self,
        node_key: &nia_node_id::NodeKey,
        _span: Span,
        expected: DefKind,
    ) -> Option<DefId> {
        let def_id = self.input.defs.def_nodes.get(node_key)?;
        let def = self.input.defs.defs.get(def_id)?;
        (def.kind == expected).then_some(def_id)
    }

    fn global_def_id(&self, def_id: DefId) -> GlobalDefId {
        GlobalDefId {
            module_id: self.input.defs.module_id,
            def_id,
        }
    }
}
