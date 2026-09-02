use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use crate::{ConstModuleInput, ConstModuleLowering};
use nia_ast::Expr;
use nia_const_ir::{
    ResolvedConstEnum, ResolvedConstEnumVariant, ResolvedConstExpr, ResolvedConstExprKind,
    ResolvedConstLocalInitializer, ResolvedConstModule,
};
use nia_defs::{DefId, DefKind};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalDefId, InternedTyId, LocalId};
use nia_item_tree::{ItemTreeNode, ItemTreeNodeKind};
use nia_local_resolve::LocalKind;
use nia_node_id::VersionedNodeKey;
use nia_sema_ir::{SemanticUseTable, SemanticValueUse};
use nia_span::Span;

/// Lowers active module const expressions into identity-resolved const IR.
///
/// The active item tree determines which definitions participate in the
/// result; unresolved names are retained as diagnostics instead of being
/// guessed from their source spelling.
pub fn lower_module_const(input: ConstModuleInput<'_>) -> ConstModuleLowering {
    let mut lowerer = ConstModuleLowerer {
        input,
        module: ResolvedConstModule::default(),
        diagnostics: Vec::new(),
    };
    lowerer.lower_module();
    ConstModuleLowering {
        module: Arc::new(lowerer.module),
        diagnostics: lowerer.diagnostics,
    }
}
struct ConstModuleLowerer<'a> {
    input: ConstModuleInput<'a>,
    module: ResolvedConstModule,
    diagnostics: Vec<Diagnostic>,
}

impl ConstModuleLowerer<'_> {
    fn lower_module(&mut self) {
        for item in &self.input.active_item_tree.items {
            match &item.kind {
                ItemTreeNodeKind::Enum(item_enum) => self.lower_enum(item, item_enum),
                ItemTreeNodeKind::Binding(binding) if binding.is_const() => {
                    self.lower_global_initializer(item.span, binding)
                }
                ItemTreeNodeKind::Function(function) if function.is_const => {
                    self.lower_function(function, DefKind::Function)
                }
                ItemTreeNodeKind::Trait(item_trait) => {
                    for method in &item_trait.methods {
                        if method.function.is_const && method.function.body.is_some() {
                            self.lower_function(&method.function, DefKind::TraitMethod);
                        }
                    }
                }
                ItemTreeNodeKind::Extend(extend) => {
                    for associated_value in &extend.associated_values {
                        if associated_value.binding.value.is_none() {
                            continue;
                        }
                        if extend.generics.is_empty() {
                            self.lower_global_initializer(
                                associated_value.span,
                                &associated_value.binding,
                            );
                        } else {
                            self.lower_deferred_global_initializer(
                                associated_value.span,
                                &associated_value.binding,
                            );
                        }
                    }
                    for method in &extend.methods {
                        if method.function.is_const {
                            self.lower_function(&method.function, DefKind::Method);
                        }
                    }
                }
                _ => {}
            }
        }
        for (local_id, local) in self.input.locals.locals.iter() {
            if local.kind == LocalKind::ConstBinding
                && let Some((expr, explicit_type)) = self.local_initializer(local_id)
                && let Some(value) = self.lower_expr(&expr)
            {
                self.module.insert_local_initializer(
                    local_id,
                    ResolvedConstLocalInitializer::new(explicit_type, value),
                );
            }
        }
        for (id, expr) in self.input.const_exprs {
            if let Some(lowered) = self.lower_expr(expr) {
                self.module.insert_const_expr(*id, lowered);
            }
        }
    }

    fn lower_enum(&mut self, item: &ItemTreeNode, item_enum: &nia_ast::EnumItem) {
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
                Some(ResolvedConstEnumVariant::new(
                    self.global_def_id(variant_id),
                    variant.span,
                    value,
                ))
            })
            .collect();
        self.module.push_enum(ResolvedConstEnum::new(
            self.global_def_id(enum_id),
            item.span,
            variants,
        ));
    }

    fn lower_global_initializer(&mut self, item_span: Span, binding: &nia_ast::BindingItem) {
        let Some(def_id) = self.def_id_for_node(&binding.node_key, item_span, DefKind::Const)
        else {
            return;
        };
        let value = binding.value.as_ref();
        let Some(value) = value else {
            if let Some(builtin) = self
                .input
                .signatures
                .consts
                .get(&def_id)
                .and_then(|signature| signature.builtin)
            {
                self.module.insert_global_initializer(
                    self.global_def_id(def_id),
                    ResolvedConstExpr::from_parts(
                        item_span,
                        ResolvedConstExprKind::BuiltinConstValue(builtin),
                    ),
                );
            }
            return;
        };
        let expected_type = self
            .input
            .signatures
            .consts
            .get(&def_id)
            .and_then(|signature| signature.explicit_type);
        if let Some(value) =
            self.lower_expr_with_expected(value, expected_type, binding.ty.as_ref())
        {
            self.module
                .insert_global_initializer(self.global_def_id(def_id), value);
        }
    }

    fn lower_deferred_global_initializer(
        &mut self,
        item_span: Span,
        binding: &nia_ast::BindingItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&binding.node_key, item_span, DefKind::Const)
        else {
            return;
        };
        let value = binding.value.as_ref();
        let Some(value) = value else {
            return;
        };
        let expected_type = self
            .input
            .signatures
            .consts
            .get(&def_id)
            .and_then(|signature| signature.explicit_type);
        if let Some(value) =
            self.lower_expr_with_expected(value, expected_type, binding.ty.as_ref())
        {
            self.module
                .insert_deferred_global_initializer(self.global_def_id(def_id), value);
        }
    }

    fn lower_function(&mut self, function: &nia_ast::FunctionItem, kind: DefKind) {
        if function.body.is_none() {
            return;
        }
        let Some(def_id) = self.def_id_for_node(&function.node_key, function.span, kind) else {
            return;
        };
        let function_locals = self.function_locals(function);
        let semantic_uses = self.semantic_uses_with_allowed_locals(&function_locals);
        let expected_type = self
            .input
            .signatures
            .functions
            .get(&def_id)
            .map(|signature| signature.return_type);
        let (aggregate_types, omitted_members) = function
            .body
            .as_ref()
            .and_then(|body| body.tail.as_deref())
            .map(|tail| {
                self.omitted_constructor_maps(
                    tail,
                    expected_type,
                    function.return_type.as_ref(),
                )
            })
            .unwrap_or_default();
        let context = nia_const_ir::ResolvedConstLowerInputs::new(&semantic_uses)
            .with_symbols(self.input.symbols)
            .with_omitted_constructor_maps(&aggregate_types, &omitted_members);
        match nia_const_ir::lower_function_resolved_with_context(function.span, function, &context)
        {
            Ok(function) => {
                self.module
                    .insert_function(self.global_def_id(def_id), function);
            }
            Err(err) => self.diagnostics.push(Diagnostic::user_error_at(
                codes::CONST,
                err.span,
                err.message,
            )),
        }
    }

    fn lower_expr(&mut self, expr: &Expr) -> Option<ResolvedConstExpr> {
        self.lower_expr_with_expected(expr, None, None)
    }

    fn lower_expr_with_expected(
        &mut self,
        expr: &Expr,
        expected_type: Option<InternedTyId>,
        expected_ref: Option<&nia_ast::TypeRef>,
    ) -> Option<ResolvedConstExpr> {
        let mut allowed_locals = HashSet::new();
        self.collect_expr_locals(expr, &mut allowed_locals);
        let semantic_uses = self.semantic_uses_with_allowed_locals(&allowed_locals);
        let (aggregate_types, omitted_members) =
            self.omitted_constructor_maps(expr, expected_type, expected_ref);
        let context = nia_const_ir::ResolvedConstLowerInputs::new(&semantic_uses)
            .with_symbols(self.input.symbols)
            .with_omitted_constructor_maps(&aggregate_types, &omitted_members);
        match nia_const_ir::lower_expr_resolved_with_context(expr, &context) {
            Ok(expr) => Some(expr),
            Err(err) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::CONST,
                    err.span,
                    err.message,
                ));
                None
            }
        }
    }

    fn omitted_constructor_maps(
        &self,
        expr: &Expr,
        expected_type: Option<InternedTyId>,
        expected_ref: Option<&nia_ast::TypeRef>,
    ) -> (
        HashMap<VersionedNodeKey, InternedTyId>,
        HashMap<VersionedNodeKey, GlobalDefId>,
    ) {
        let mut aggregate_types = HashMap::new();
        let mut omitted_members = HashMap::new();
        if let (Some(expected_type), nia_ast::ExprKind::OmittedAggregateLiteral { .. }) =
            (expected_type, &expr.kind)
        {
            aggregate_types.insert(expr.node_key.clone(), expected_type);
        }
        let enum_id =
            expected_ref.and_then(|ty| {
                self.input
                    .semantic_uses
                    .node_type_prefix(&ty.node_key)
                    .or_else(|| match &ty.kind {
                        nia_ast::TypeKind::Path { segments } if segments.len() == 1 => {
                            let nia_ast::PathSegmentKind::Name(name) = segments[0].kind else {
                                return None;
                            };
                            self.input.defs.module_scope.types.get(&name).map(|def_id| {
                                GlobalDefId {
                                    module_id: self.input.defs.module_id,
                                    def_id,
                                }
                            })
                        }
                        _ => None,
                    })
            });
        let variant_for = |name: &nia_symbol::SymbolId| {
            let enum_id = enum_id?;
            let scope = self.input.defs.scopes.enum_members.get(&enum_id.def_id)?;
            let def_id = scope.variants.get(name)?;
            Some(GlobalDefId {
                module_id: enum_id.module_id,
                def_id,
            })
        };
        let mut record_member = |member: &Expr| {
            if let nia_ast::ExprKind::OmittedMember { name } = &member.kind
                && let Some(variant_id) = variant_for(name)
            {
                omitted_members.insert(member.node_key.clone(), variant_id);
            }
        };
        match &expr.kind {
            nia_ast::ExprKind::OmittedMember { .. } => record_member(expr),
            nia_ast::ExprKind::Call { callee, .. } => record_member(callee),
            nia_ast::ExprKind::QualifiedStructLiteral { target, .. } => record_member(target),
            _ => {}
        }
        (aggregate_types, omitted_members)
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
                            .is_some_and(|local| local.kind == LocalKind::ConstBinding) =>
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
        builder.extend_node_associated_const_projections(
            self.input
                .semantic_uses
                .node_associated_const_projections
                .iter()
                .map(|(key, projection)| (key.clone(), projection.clone())),
        );
        builder.extend_node_const_generic_uses(
            self.input
                .semantic_uses
                .node_const_generic_uses
                .iter()
                .map(|(key, name)| (key.clone(), *name)),
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
        builder.extend_node_type_prefixes(
            self.input
                .semantic_uses
                .node_type_prefixes
                .iter()
                .map(|(key, def_id)| (key.clone(), *def_id)),
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
                self.collect_pattern_locals(&binding.pattern, out);
                if let Some(value) = &binding.value {
                    self.collect_expr_locals(value, out);
                }
            }
            nia_ast::StmtKind::Static(binding) => {
                if let Some(value) = &binding.value {
                    self.collect_expr_locals(value, out);
                }
            }
            nia_ast::StmtKind::Expr(expr)
            | nia_ast::StmtKind::Return(Some(expr))
            | nia_ast::StmtKind::Defer(expr) => self.collect_expr_locals(expr, out),
            nia_ast::StmtKind::ForIn(for_stmt) => {
                self.collect_pattern_locals(&for_stmt.pattern, out);
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
            nia_ast::ExprKind::Tuple(elems) => {
                for elem in elems {
                    self.collect_expr_locals(elem, out);
                }
            }
            nia_ast::ExprKind::Closure { captures, body, .. } => {
                for capture in captures {
                    self.collect_expr_locals(&capture.value, out);
                }
                self.collect_expr_locals(body, out);
            }
            nia_ast::ExprKind::TupleField { lhs, .. } => self.collect_expr_locals(lhs, out),
            nia_ast::ExprKind::ArrayLiteral { elems } => match elems {
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
            nia_ast::ExprKind::TypedStructLiteral { fields, .. } => {
                for field in fields {
                    self.collect_expr_locals(&field.value, out);
                }
            }
            nia_ast::ExprKind::QualifiedStructLiteral { target, fields } => {
                self.collect_expr_locals(target, out);
                for field in fields {
                    self.collect_expr_locals(&field.value, out);
                }
            }
            nia_ast::ExprKind::OmittedAggregateLiteral { fields } => {
                for field in fields {
                    self.collect_expr_locals(&field.value, out);
                }
            }
            nia_ast::ExprKind::OmittedMember { .. } => {}
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
                self.collect_pattern_locals(&if_pattern.pattern, out);
                self.collect_block_locals(&if_pattern.then_branch, out);
                if let Some(else_branch) = if_pattern.else_branch.as_deref() {
                    self.collect_expr_locals(else_branch, out);
                }
            }
            nia_ast::ExprKind::Match(matched) => {
                self.collect_expr_locals(&matched.target, out);
                for arm in &matched.arms {
                    for pattern in &arm.patterns {
                        self.collect_pattern_locals(pattern, out);
                    }
                    match &arm.body {
                        nia_ast::MatchArmBody::Expr(expr) => self.collect_expr_locals(expr, out),
                        nia_ast::MatchArmBody::Stmt(stmt) => self.collect_stmt_locals(stmt, out),
                        nia_ast::MatchArmBody::Block(block) => {
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
            | nia_ast::ExprKind::SelfValue
            | nia_ast::ExprKind::PathRoot(_)
            | nia_ast::ExprKind::Underscore
            | nia_ast::ExprKind::TypeTarget { .. }
            | nia_ast::ExprKind::TraitTarget { .. } => {}
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
            nia_ast::PatternKind::Pointer(pattern)
            | nia_ast::PatternKind::MutPointer(pattern)
            | nia_ast::PatternKind::OptionalSome(pattern)
            | nia_ast::PatternKind::ErrorOk(pattern)
            | nia_ast::PatternKind::ErrorErr(pattern) => self.collect_pattern_locals(pattern, out),
            nia_ast::PatternKind::Tuple(patterns) => {
                for pattern in patterns {
                    self.collect_pattern_locals(pattern, out);
                }
            }
            nia_ast::PatternKind::Nominal {
                constructor: variant,
                fields,
            } => {
                self.collect_expr_locals(variant, out);
                match fields {
                    nia_ast::NominalPatternFields::Tuple(fields) => {
                        for field in fields {
                            self.collect_pattern_locals(field, out);
                        }
                    }
                    nia_ast::NominalPatternFields::Named { fields, .. } => {
                        for field in fields {
                            self.collect_pattern_locals(&field.pattern, out);
                        }
                    }
                }
            }
            nia_ast::PatternKind::Expr(expr) => self.collect_expr_locals(expr, out),
            nia_ast::PatternKind::Range { start, end, .. } => {
                self.collect_expr_locals(start, out);
                self.collect_expr_locals(end, out);
            }
            nia_ast::PatternKind::Wildcard | nia_ast::PatternKind::OptionalNull => {}
        }
    }

    fn pattern_local_id(&self, pattern: &nia_ast::Pattern) -> Option<LocalId> {
        match &pattern.kind {
            nia_ast::PatternKind::Bind { node_key, .. } => {
                self.input.semantic_uses.node_local_def(node_key)
            }
            nia_ast::PatternKind::Pointer(pattern) | nia_ast::PatternKind::MutPointer(pattern) => {
                self.pattern_local_id(pattern)
            }
            _ => None,
        }
    }

    fn local_initializer(&self, local_id: LocalId) -> Option<(Expr, Option<InternedTyId>)> {
        self.input
            .active_item_tree
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemTreeNodeKind::Function(function) => function
                    .body
                    .as_ref()
                    .and_then(|body| self.local_initializer_in_block(local_id, body)),
                ItemTreeNodeKind::Extend(extend) => extend.methods.iter().find_map(|method| {
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
                    if self.pattern_local_id(&binding.pattern) == Some(local_id) =>
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
        node_key: &VersionedNodeKey,
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
