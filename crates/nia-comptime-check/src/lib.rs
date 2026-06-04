// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_ast::{Expr, ItemKind, Module};
use nia_comptime_engine::{ComptimeEnv, ComptimeError};
use nia_comptime_ir::{
    ComptimeEnum, ComptimeEnumVariant, ComptimeExpr, ComptimeModule, ComptimeNameResolution,
};
use nia_defs::{DefCollection, DefId, DefKind};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalConstExprId, GlobalDefId, LayoutBuiltin, LocalId, ModuleId};
use nia_item_signatures::ItemSignatures;
use nia_local_resolve::{LocalKind, LocalResolution, LocalUse};
use nia_span::Span;
use nia_target_config::TargetConfig;
use nia_ty::{PrimitiveTy, TyInterner, TyKind};
use nia_value_resolve::{ValueNameResolution, ValueResolution};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ComptimeCheck {
    pub values: HashMap<ComptimeKey, ComptimeValue>,
    pub enum_values: HashMap<DefId, ComptimeValue>,
    pub array_lengths: HashMap<GlobalConstExprId, u64>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComptimeKey {
    Global(GlobalDefId),
    Local(LocalId),
}

pub use nia_comptime_engine::ComptimeValue;

#[derive(Debug, Clone, Copy)]
pub struct ComptimeInput<'a> {
    pub module: &'a ComptimeModule,
    pub defs: &'a DefCollection,
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub signatures: &'a ItemSignatures,
    pub interner: &'a TyInterner,
    pub type_uses: &'a HashMap<Span, nia_ids::InternedTyId>,
    pub normalized: &'a HashMap<nia_ids::InternedTyId, nia_ids::InternedTyId>,
    pub target: &'a TargetConfig,
    pub program: ComptimeProgramContext<'a>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ComptimeModuleLowering {
    pub module: ComptimeModule,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy)]
pub struct ComptimeModuleInput<'a> {
    pub module: &'a Module,
    pub defs: &'a DefCollection,
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub const_exprs: &'a HashMap<GlobalConstExprId, Expr>,
}

#[derive(Debug, Clone, Copy)]
pub struct ComptimeProgramContext<'a> {
    pub modules: Option<&'a HashMap<ModuleId, ComptimeModule>>,
    pub defs: Option<&'a HashMap<ModuleId, DefCollection>>,
}

impl<'a> ComptimeProgramContext<'a> {
    pub fn empty() -> Self {
        Self {
            modules: None,
            defs: None,
        }
    }
}

pub fn lower_module_comptime(input: ComptimeModuleInput<'_>) -> ComptimeModuleLowering {
    let mut lowerer = ComptimeModuleLowerer {
        input,
        module: ComptimeModule::default(),
        diagnostics: Vec::new(),
    };
    lowerer.lower_module();
    ComptimeModuleLowering {
        module: lowerer.module,
        diagnostics: lowerer.diagnostics,
    }
}

pub fn check_module_comptime(input: ComptimeInput<'_>) -> ComptimeCheck {
    let mut analyzer = Analyzer {
        input,
        values: HashMap::new(),
        call_locals: Vec::new(),
        enum_values: HashMap::new(),
        array_lengths: HashMap::new(),
        diagnostics: Vec::new(),
        active: HashSet::new(),
    };
    analyzer.analyze_module();
    ComptimeCheck {
        values: analyzer.values,
        enum_values: analyzer.enum_values,
        array_lengths: analyzer.array_lengths,
        diagnostics: analyzer.diagnostics,
    }
}

struct ComptimeModuleLowerer<'a> {
    input: ComptimeModuleInput<'a>,
    module: ComptimeModule,
    diagnostics: Vec<Diagnostic>,
}

impl ComptimeModuleLowerer<'_> {
    fn lower_module(&mut self) {
        for item in &self.input.module.items {
            match &item.kind {
                ItemKind::Enum(item_enum) => self.lower_enum(item.span, item_enum),
                ItemKind::Binding(binding) if binding.is_comptime => {
                    self.lower_global_initializer(item.span, binding.value.as_ref())
                }
                ItemKind::Function(function) if function.is_comptime => {
                    self.lower_function(item.span, function)
                }
                ItemKind::Extend(extend) => {
                    for method in &extend.methods {
                        if method.function.is_comptime {
                            self.lower_function(method.function.span, &method.function);
                        }
                    }
                }
                _ => {}
            }
        }
        for (local_id, local) in self.input.locals.locals.iter() {
            if local.kind == LocalKind::ComptimeBinding
                && let Some(expr) = self.local_initializer(local_id)
                && let Some(lowered) = self.lower_expr(&expr)
            {
                self.module.local_initializers.insert(local_id, lowered);
            }
        }
        for (id, expr) in self.input.const_exprs {
            if let Some(lowered) = self.lower_expr(expr) {
                self.module.const_exprs.insert(*id, lowered);
            }
        }
    }

    fn lower_enum(&mut self, item_span: Span, item_enum: &nia_ast::EnumItem) {
        let Some(enum_id) = self.def_id_for_span(item_span, DefKind::Enum) else {
            return;
        };
        let variants = item_enum
            .variants
            .iter()
            .filter_map(|variant| {
                let variant_id = self.def_id_for_span(variant.span, DefKind::EnumVariant)?;
                let value = variant
                    .value
                    .as_ref()
                    .and_then(|expr| self.lower_expr(expr));
                Some(ComptimeEnumVariant {
                    def_id: self.global_def_id(variant_id),
                    span: variant.span,
                    value,
                })
            })
            .collect();
        self.module.enums.push(ComptimeEnum {
            def_id: self.global_def_id(enum_id),
            span: item_span,
            variants,
        });
    }

    fn lower_global_initializer(&mut self, item_span: Span, value: Option<&Expr>) {
        let Some(def_id) = self.def_id_for_span(item_span, DefKind::Comptime) else {
            return;
        };
        let Some(value) = value else {
            return;
        };
        if let Some(value) = self.lower_expr(value) {
            self.module
                .global_initializers
                .insert(self.global_def_id(def_id), value);
        }
    }

    fn lower_function(&mut self, function_span: Span, function: &nia_ast::FunctionItem) {
        let Some(def_id) = self.def_id_for_span(function_span, DefKind::Function) else {
            return;
        };
        let function_locals = self.function_locals(function);
        let name_resolution =
            |span| self.name_resolution_with_allowed_locals(span, &function_locals);
        let local_id = |span| self.input.locals.local_defs.get(&span).copied();
        let context = nia_comptime_ir::ComptimeLowerContext {
            name_resolution: Some(&name_resolution),
            local_id: Some(&local_id),
        };
        match nia_comptime_ir::lower_function_with_context(function_span, function, &context) {
            Ok(function) => {
                self.module
                    .functions
                    .insert(self.global_def_id(def_id), function);
            }
            Err(err) => self
                .diagnostics
                .push(Diagnostic::error(err.span, err.message)),
        }
    }

    fn lower_expr(&mut self, expr: &Expr) -> Option<ComptimeExpr> {
        let allowed_locals = HashSet::new();
        let name_resolution =
            |span| self.name_resolution_with_allowed_locals(span, &allowed_locals);
        let local_id = |span| self.input.locals.local_defs.get(&span).copied();
        let context = nia_comptime_ir::ComptimeLowerContext {
            name_resolution: Some(&name_resolution),
            local_id: Some(&local_id),
        };
        match nia_comptime_ir::lower_expr_with_context(expr, &context) {
            Ok(expr) => Some(expr),
            Err(err) => {
                self.diagnostics
                    .push(Diagnostic::error(err.span, err.message));
                None
            }
        }
    }

    fn name_resolution_with_allowed_locals(
        &self,
        span: Span,
        allowed_locals: &HashSet<LocalId>,
    ) -> Option<ComptimeNameResolution> {
        if let Some(local_id) = self.local_comptime_use(span) {
            return Some(ComptimeNameResolution::Local(local_id));
        }
        if let Some(local_id) = self.local_use(span)
            && allowed_locals.contains(&local_id)
        {
            return Some(ComptimeNameResolution::Local(local_id));
        }
        if let Some(global_id) = self.global_value_use(span) {
            return Some(ComptimeNameResolution::Global(global_id));
        }
        None
    }

    fn function_locals(&self, function: &nia_ast::FunctionItem) -> HashSet<LocalId> {
        let mut locals = HashSet::new();
        for param in &function.params {
            if let Some(local_id) = self.input.locals.local_defs.get(&param.span).copied() {
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
                if let Some(local_id) = self.input.locals.local_defs.get(&stmt.span).copied() {
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
                if let Some(local_id) = self
                    .input
                    .locals
                    .local_defs
                    .get(&for_stmt.binding.span)
                    .copied()
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
            | nia_ast::ExprKind::CString(_)
            | nia_ast::ExprKind::Char(_)
            | nia_ast::ExprKind::ByteChar(_)
            | nia_ast::ExprKind::Raw(_)
            | nia_ast::ExprKind::Bool(_)
            | nia_ast::ExprKind::Null
            | nia_ast::ExprKind::Ident(_)
            | nia_ast::ExprKind::Underscore
            | nia_ast::ExprKind::Builtin { .. }
            | nia_ast::ExprKind::TypeTarget { .. } => {}
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

    fn collect_switch_pattern_locals(
        &self,
        pattern: &nia_ast::SwitchPattern,
        out: &mut HashSet<LocalId>,
    ) {
        match pattern {
            nia_ast::SwitchPattern::Expr(expr) => self.collect_expr_locals(expr, out),
            nia_ast::SwitchPattern::Range { start, end, .. } => {
                self.collect_expr_locals(start, out);
                self.collect_expr_locals(end, out);
            }
            nia_ast::SwitchPattern::Default
            | nia_ast::SwitchPattern::OptionalSome { .. }
            | nia_ast::SwitchPattern::OptionalNull { .. }
            | nia_ast::SwitchPattern::ErrorOk { .. }
            | nia_ast::SwitchPattern::ErrorErr { .. } => {}
        }
    }

    fn local_comptime_use(&self, span: Span) -> Option<LocalId> {
        let Some(LocalUse::Local(local_id)) = self.input.locals.uses.get(&span) else {
            return None;
        };
        let local = self.input.locals.locals.get(*local_id)?;
        (local.kind == LocalKind::ComptimeBinding).then_some(*local_id)
    }

    fn local_use(&self, span: Span) -> Option<LocalId> {
        let Some(LocalUse::Local(local_id)) = self.input.locals.uses.get(&span) else {
            return None;
        };
        Some(*local_id)
    }

    fn global_value_use(&self, span: Span) -> Option<GlobalDefId> {
        if let Some(global_id) = self.input.values.qualified_values.get(&span).copied() {
            return Some(global_id);
        }
        let Some(ValueNameResolution::Def(def_id)) = self.input.values.names.get(&span) else {
            return None;
        };
        Some(self.global_def_id(*def_id))
    }

    fn local_initializer(&self, local_id: LocalId) -> Option<Expr> {
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
    ) -> Option<Expr> {
        for stmt in &block.stmts {
            match &stmt.kind {
                nia_ast::StmtKind::Binding(binding)
                    if self.input.locals.local_defs.get(&stmt.span).copied() == Some(local_id) =>
                {
                    return binding.value.clone();
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

    fn def_id_for_span(&self, span: Span, expected: DefKind) -> Option<DefId> {
        let def_id = self.input.defs.def_spans.get(span)?;
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

struct Analyzer<'a> {
    input: ComptimeInput<'a>,
    values: HashMap<ComptimeKey, ComptimeValue>,
    call_locals: Vec<ComptimeCallFrame>,
    enum_values: HashMap<DefId, ComptimeValue>,
    array_lengths: HashMap<GlobalConstExprId, u64>,
    diagnostics: Vec<Diagnostic>,
    active: HashSet<ComptimeKey>,
}

#[derive(Debug, Clone, Default)]
struct ComptimeCallFrame {
    locals: HashMap<LocalId, ComptimeValue>,
    names: HashMap<String, ComptimeValue>,
}

impl Analyzer<'_> {
    fn analyze_module(&mut self) {
        let enums = self.input.module.enums.clone();
        for item_enum in &enums {
            self.eval_enum(item_enum);
        }
        let global_initializers = self
            .input
            .module
            .global_initializers
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for global_id in global_initializers {
            let span = self
                .global_defs(global_id.module_id)
                .and_then(|defs| defs.defs.get(global_id.def_id))
                .map(|def| def.span)
                .unwrap_or(Span::new(0, 0));
            let _ = self.eval_key(ComptimeKey::Global(global_id), span);
        }
        let local_initializers = self
            .input
            .module
            .local_initializers
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for local_id in local_initializers {
            let span = self
                .input
                .locals
                .locals
                .get(local_id)
                .map(|local| local.span)
                .unwrap_or(Span::new(0, 0));
            let _ = self.eval_key(ComptimeKey::Local(local_id), span);
        }
        let const_exprs = self.input.module.const_exprs.clone();
        for (id, expr) in const_exprs {
            if let Some(value) = self.eval_array_len_expr(&expr) {
                self.array_lengths.insert(id, value);
            }
        }
    }

    fn eval_enum(&mut self, item_enum: &ComptimeEnum) {
        let range = self.enum_backing_range(item_enum.def_id.def_id);
        let mut next_value = 0i128;
        for variant in &item_enum.variants {
            let value = if let Some(expr) = variant.value.as_ref() {
                match nia_comptime_engine::eval_comptime_int_expr(expr, self) {
                    Ok(value) => value,
                    Err(err) => {
                        self.push_engine_error(err);
                        next_value = next_value.saturating_add(1);
                        continue;
                    }
                }
            } else {
                next_value
            };
            if let Some((min, max)) = range
                && (value < min || value > max)
            {
                self.diagnostics.push(Diagnostic::error(
                    variant.span,
                    format!("enum variant value {value} is out of range for backing type"),
                ));
            }
            self.enum_values
                .insert(variant.def_id.def_id, ComptimeValue::Int(value));
            next_value = value.saturating_add(1);
        }
    }

    fn enum_backing_range(&self, enum_id: DefId) -> Option<(i128, i128)> {
        let signature = self.input.signatures.enums.get(&enum_id)?;
        let Some(TyKind::Primitive(primitive)) = self.input.interner.get(signature.backing_type)
        else {
            return None;
        };
        integer_range(*primitive)
    }

    fn eval_array_len_expr(&mut self, expr: &ComptimeExpr) -> Option<u64> {
        match nia_comptime_engine::eval_comptime_array_len_expr(expr, self) {
            Ok(value) => Some(value),
            Err(err) => {
                self.push_engine_error(err);
                None
            }
        }
    }

    fn eval_key(&mut self, key: ComptimeKey, span: Span) -> Option<ComptimeValue> {
        if let Some(value) = self.values.get(&key).cloned() {
            return Some(value);
        }
        if !self.active.insert(key) {
            self.diagnostics
                .push(Diagnostic::error(span, "cyclic comptime dependency"));
            return None;
        }
        let result = self.initializer_for_key(key).cloned().and_then(|expr| {
            match nia_comptime_engine::eval_comptime_expr(&expr, self) {
                Ok(value) => Some(value),
                Err(err) => {
                    self.push_engine_error(err);
                    None
                }
            }
        });
        self.active.remove(&key);
        if let Some(value) = result.clone() {
            self.values.insert(key, value);
        }
        result
    }

    fn push_engine_error(&mut self, err: ComptimeError) {
        self.diagnostics
            .push(Diagnostic::error(err.span, err.message));
    }

    fn initializer_for_key(&self, key: ComptimeKey) -> Option<&ComptimeExpr> {
        match key {
            ComptimeKey::Global(global_id) => self.global_initializer(global_id),
            ComptimeKey::Local(local_id) => self.local_initializer(local_id),
        }
    }

    fn global_initializer(&self, global_id: GlobalDefId) -> Option<&ComptimeExpr> {
        if global_id.module_id == self.input.defs.module_id {
            self.input.module.global_initializers.get(&global_id)
        } else {
            self.input
                .program
                .modules?
                .get(&global_id.module_id)?
                .global_initializers
                .get(&global_id)
        }
    }

    fn global_defs(&self, module_id: ModuleId) -> Option<&DefCollection> {
        if module_id == self.input.defs.module_id {
            Some(self.input.defs)
        } else {
            self.input.program.defs?.get(&module_id)
        }
    }

    fn local_initializer(&self, local_id: LocalId) -> Option<&ComptimeExpr> {
        self.input.module.local_initializers.get(&local_id)
    }

    fn local_comptime_use(&self, span: Span) -> Option<LocalId> {
        let Some(LocalUse::Local(local_id)) = self.input.locals.uses.get(&span) else {
            return None;
        };
        let local = self.input.locals.locals.get(*local_id)?;
        (local.kind == LocalKind::ComptimeBinding).then_some(*local_id)
    }

    fn local_use(&self, span: Span) -> Option<LocalId> {
        let Some(LocalUse::Local(local_id)) = self.input.locals.uses.get(&span) else {
            return None;
        };
        Some(*local_id)
    }

    fn call_local_value(&self, local_id: LocalId) -> Option<ComptimeValue> {
        self.call_locals
            .iter()
            .rev()
            .find_map(|frame| frame.locals.get(&local_id).cloned())
    }

    fn call_local_name_value(&self, name: &str) -> Option<ComptimeValue> {
        self.call_locals
            .iter()
            .rev()
            .find_map(|frame| frame.names.get(name).cloned())
    }

    fn global_comptime_use(&self, span: Span) -> Option<GlobalDefId> {
        if let Some(global_id) = self.input.values.qualified_values.get(&span).copied() {
            let def = self.def_kind_of(global_id)?;
            if def == DefKind::Comptime {
                return Some(global_id);
            }
            return None;
        }
        let Some(ValueNameResolution::Def(def_id)) = self.input.values.names.get(&span) else {
            return None;
        };
        let def = self.input.defs.defs.get(*def_id)?;
        (def.kind == DefKind::Comptime).then_some(self.global_def_id(*def_id))
    }

    fn def_kind_of(&self, global_id: GlobalDefId) -> Option<DefKind> {
        self.global_defs(global_id.module_id)?
            .defs
            .get(global_id.def_id)
            .map(|def| def.kind)
    }

    fn global_def_id(&self, def_id: DefId) -> GlobalDefId {
        GlobalDefId {
            module_id: self.input.defs.module_id,
            def_id,
        }
    }

    fn comptime_function(&self, callee: &nia_comptime_engine::ComptimeExpr) -> Option<GlobalDefId> {
        match &callee.kind {
            nia_comptime_engine::ComptimeExprKind::Ident {
                resolution: Some(nia_comptime_engine::ComptimeNameResolution::Global(global_id)),
                ..
            }
            | nia_comptime_engine::ComptimeExprKind::Qualified {
                resolution: Some(nia_comptime_engine::ComptimeNameResolution::Global(global_id)),
                ..
            } => {
                return (self.def_kind_of(*global_id) == Some(DefKind::Function))
                    .then_some(*global_id);
            }
            _ => {}
        }
        if let Some(global_id) = self
            .input
            .values
            .qualified_values
            .get(&callee.span)
            .copied()
        {
            if self.def_kind_of(global_id) == Some(DefKind::Function) {
                return Some(global_id);
            }
            return None;
        }
        let nia_comptime_engine::ComptimeExprKind::Ident { .. } = &callee.kind else {
            return None;
        };
        let Some(ValueNameResolution::Def(def_id)) = self.input.values.names.get(&callee.span)
        else {
            return None;
        };
        let def = self.input.defs.defs.get(*def_id)?;
        if def.kind != DefKind::Function {
            return None;
        }
        Some(GlobalDefId {
            module_id: self.input.defs.module_id,
            def_id: *def_id,
        })
    }

    fn comptime_function_body(
        &self,
        def_id: GlobalDefId,
    ) -> Option<&nia_comptime_ir::ComptimeFunction> {
        if def_id.module_id == self.input.defs.module_id {
            self.input.module.functions.get(&def_id)
        } else {
            self.input
                .program
                .modules?
                .get(&def_id.module_id)?
                .functions
                .get(&def_id)
        }
    }

    fn ty_for_span(&self, span: Span) -> Option<nia_ids::InternedTyId> {
        self.input.type_uses.get(&span).copied()
    }

    fn resolve_layout_builtin_for_ty(
        &self,
        span: Span,
        builtin: LayoutBuiltin,
        ty: nia_ids::InternedTyId,
    ) -> Result<ComptimeValue, ComptimeError> {
        let current_lengths = ComptimeCheck {
            values: self.values.clone(),
            enum_values: self.enum_values.clone(),
            array_lengths: self.array_lengths.clone(),
            diagnostics: Vec::new(),
        };
        let array_lengths = |id| current_lengths.array_lengths.get(&id).copied();
        let layouts = nia_layout::compute_layouts_with_normalized_types(
            self.input.defs,
            self.input.interner,
            self.input.signatures,
            self.input.normalized,
            &array_lengths,
            nia_layout::TargetDataLayout::LP64,
        );
        let ty = self.input.normalized.get(&ty).copied().unwrap_or(ty);
        let Some(layout) = layouts.types.get(&ty) else {
            return Err(ComptimeError {
                span,
                message: format!(
                    "cannot compute layout for comptime builtin `@{}`",
                    builtin.name()
                ),
            });
        };
        let value = match builtin {
            LayoutBuiltin::Size => layout.size,
            LayoutBuiltin::Align => layout.align,
        };
        Ok(ComptimeValue::Int(value as i128))
    }
}

impl ComptimeEnv for Analyzer<'_> {
    fn resolve_ident(&mut self, span: Span, name: &str) -> Result<ComptimeValue, ComptimeError> {
        if let Some(value) = self.call_local_name_value(name) {
            return Ok(value);
        }
        if let Some(local_id) = self.local_use(span)
            && let Some(value) = self.call_local_value(local_id)
        {
            return Ok(value);
        }
        let key = if let Some(local_id) = self.local_comptime_use(span) {
            ComptimeKey::Local(local_id)
        } else if let Some(global_id) = self.global_comptime_use(span) {
            ComptimeKey::Global(global_id)
        } else {
            return Err(ComptimeError {
                span,
                message: format!("comptime expression can only use comptime bindings: `{name}`"),
            });
        };
        self.eval_key(key, span).ok_or_else(|| ComptimeError {
            span,
            message: format!("failed to evaluate comptime value `{name}`"),
        })
    }

    fn resolve_name_resolution(
        &mut self,
        span: Span,
        resolution: nia_comptime_engine::ComptimeNameResolution,
        name: &str,
    ) -> Result<ComptimeValue, ComptimeError> {
        match resolution {
            nia_comptime_engine::ComptimeNameResolution::Local(local_id) => {
                if let Some(value) = self.call_local_value(local_id) {
                    return Ok(value);
                }
                if let Some(value) = self.call_local_name_value(name) {
                    return Ok(value);
                }
                self.eval_key(ComptimeKey::Local(local_id), span)
                    .ok_or_else(|| ComptimeError {
                        span,
                        message: format!("failed to evaluate comptime value `{name}`"),
                    })
            }
            nia_comptime_engine::ComptimeNameResolution::Global(global_id) => {
                if self.def_kind_of(global_id) == Some(DefKind::Comptime) {
                    return self
                        .eval_key(ComptimeKey::Global(global_id), span)
                        .ok_or_else(|| ComptimeError {
                            span,
                            message: format!("failed to evaluate comptime value `{name}`"),
                        });
                }
                Err(ComptimeError {
                    span,
                    message: format!(
                        "comptime expression can only use comptime bindings: `{name}`"
                    ),
                })
            }
        }
    }

    fn resolve_builtin_value(
        &mut self,
        span: Span,
        name: &str,
    ) -> Result<ComptimeValue, ComptimeError> {
        if name == "builtin" {
            return Ok(nia_target_config::builtin_comptime_value(self.input.target));
        }
        Err(ComptimeError {
            span,
            message: format!("unsupported builtin value in comptime expression: @{name}"),
        })
    }

    fn resolve_layout_builtin(
        &mut self,
        span: Span,
        builtin: LayoutBuiltin,
        type_arg_span: Span,
    ) -> Result<ComptimeValue, ComptimeError> {
        let Some(ty_id) = self.ty_for_span(type_arg_span) else {
            return Err(ComptimeError {
                span,
                message: format!(
                    "cannot resolve type argument for comptime builtin `@{}`",
                    builtin.name()
                ),
            });
        };
        self.resolve_layout_builtin_for_ty(span, builtin, ty_id)
    }

    fn call_function(
        &mut self,
        span: Span,
        callee: &nia_comptime_engine::ComptimeExpr,
        args: Vec<ComptimeValue>,
    ) -> Result<ComptimeValue, ComptimeError> {
        let Some(function_id) = self.comptime_function(callee) else {
            return Err(ComptimeError {
                span,
                message: "comptime expression can only call `comptime fn`".to_string(),
            });
        };
        let Some(function) = self.comptime_function_body(function_id).cloned() else {
            return Err(ComptimeError {
                span,
                message: "comptime expression can only call `comptime fn`".to_string(),
            });
        };
        nia_comptime_engine::eval_comptime_function_call(span, &function, args, self)
    }

    fn push_function_frame(&mut self, _span: Span) -> Result<(), ComptimeError> {
        self.call_locals.push(ComptimeCallFrame::default());
        Ok(())
    }

    fn pop_function_frame(&mut self) {
        self.call_locals.pop();
    }

    fn bind_function_param(
        &mut self,
        span: Span,
        param: &nia_comptime_engine::ComptimeParam,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let Some(local_id) = param
            .local_id
            .or_else(|| self.input.locals.local_defs.get(&param.span).copied())
        else {
            return Err(ComptimeError {
                span,
                message: "failed to bind comptime function parameter".to_string(),
            });
        };
        self.bind_local_value(span, local_id, &param.name, value)
    }

    fn bind_function_local(
        &mut self,
        span: Span,
        binding: &nia_comptime_engine::ComptimeBinding,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let Some(local_id) = binding
            .local_id
            .or_else(|| self.input.locals.local_defs.get(&span).copied())
        else {
            return Err(ComptimeError {
                span,
                message: "failed to bind comptime function local".to_string(),
            });
        };
        self.bind_local_value(span, local_id, &binding.name, value)
    }

    fn bind_switch_pattern_local(
        &mut self,
        span: Span,
        name: &str,
        local_id: Option<LocalId>,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let Some(local_id) = local_id.or_else(|| self.input.locals.local_defs.get(&span).copied())
        else {
            return Err(ComptimeError {
                span,
                message: "failed to bind comptime switch pattern local".to_string(),
            });
        };
        self.bind_local_value(span, local_id, name, value)
    }
}

impl Analyzer<'_> {
    fn bind_local_value(
        &mut self,
        span: Span,
        local_id: LocalId,
        name: &str,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let Some(frame) = self.call_locals.last_mut() else {
            return Err(ComptimeError {
                span,
                message: "internal comptime function frame is missing".to_string(),
            });
        };
        frame.locals.insert(local_id, value.clone());
        frame.names.insert(name.to_string(), value);
        Ok(())
    }
}

fn integer_range(ty: PrimitiveTy) -> Option<(i128, i128)> {
    match ty {
        PrimitiveTy::I8 => Some((i8::MIN as i128, i8::MAX as i128)),
        PrimitiveTy::I16 => Some((i16::MIN as i128, i16::MAX as i128)),
        PrimitiveTy::I32 => Some((i32::MIN as i128, i32::MAX as i128)),
        PrimitiveTy::I64 => Some((i64::MIN as i128, i64::MAX as i128)),
        PrimitiveTy::I128 => Some((i128::MIN, i128::MAX)),
        PrimitiveTy::Isize => Some((isize::MIN as i128, isize::MAX as i128)),
        PrimitiveTy::U8 => Some((u8::MIN as i128, u8::MAX as i128)),
        PrimitiveTy::U16 => Some((u16::MIN as i128, u16::MAX as i128)),
        PrimitiveTy::U32 => Some((u32::MIN as i128, u32::MAX as i128)),
        PrimitiveTy::U64 => Some((u64::MIN as i128, u64::MAX as i128)),
        PrimitiveTy::U128 => Some((0, i128::MAX)),
        PrimitiveTy::Usize => Some((0, usize::MAX as i128)),
        PrimitiveTy::Bool
        | PrimitiveTy::Char
        | PrimitiveTy::F32
        | PrimitiveTy::F64
        | PrimitiveTy::Void
        | PrimitiveTy::Never => None,
    }
}
