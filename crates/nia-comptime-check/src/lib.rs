// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_ast::{Expr, ExprKind, ItemKind, Module};
use nia_comptime_engine::{ComptimeEnv, ComptimeError};
use nia_defs::{DefCollection, DefId, DefKind};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, LocalId};
use nia_item_signatures::ItemSignatures;
use nia_local_resolve::{LocalKind, LocalResolution, LocalUse};
use nia_span::Span;
use nia_ty::{ArrayLenTy, PrimitiveTy, TyInterner, TyKind};
use nia_value_resolve::{ValueNameResolution, ValueResolution};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ComptimeCheck {
    pub values: HashMap<ComptimeKey, ComptimeValue>,
    pub enum_values: HashMap<DefId, ComptimeValue>,
    pub array_lengths: HashMap<Span, u64>,
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
    pub module: &'a Module,
    pub all_modules: &'a [Module],
    pub defs: &'a DefCollection,
    pub all_defs: &'a [DefCollection],
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub signatures: &'a ItemSignatures,
    pub interner: &'a TyInterner,
}

pub fn check_module_comptime(input: ComptimeInput<'_>) -> ComptimeCheck {
    let mut analyzer = Analyzer {
        input,
        values: HashMap::new(),
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

struct Analyzer<'a> {
    input: ComptimeInput<'a>,
    values: HashMap<ComptimeKey, ComptimeValue>,
    enum_values: HashMap<DefId, ComptimeValue>,
    array_lengths: HashMap<Span, u64>,
    diagnostics: Vec<Diagnostic>,
    active: HashSet<ComptimeKey>,
}

impl Analyzer<'_> {
    fn analyze_module(&mut self) {
        for item in &self.input.module.items {
            if let ItemKind::Enum(item_enum) = &item.kind {
                self.eval_enum(item.span, item_enum);
            }
            if let ItemKind::Binding(binding) = &item.kind
                && binding.is_comptime
                && let Some(def_id) = self.def_id_for_span(item.span, DefKind::Comptime)
            {
                let key = ComptimeKey::Global(self.global_def_id(def_id));
                let _ = self.eval_key(key, item.span);
            }
        }
        for (local_id, local) in self.input.locals.locals.iter() {
            if local.kind == LocalKind::ComptimeBinding {
                let _ = self.eval_key(ComptimeKey::Local(local_id), local.span);
            }
        }
        for (_, ty) in self.input.interner.iter() {
            self.collect_array_lengths_in_ty(ty);
        }
    }

    fn eval_enum(&mut self, item_span: Span, item_enum: &nia_ast::EnumItem) {
        let Some(enum_id) = self.def_id_for_span(item_span, DefKind::Enum) else {
            return;
        };
        let range = self.enum_backing_range(enum_id);
        let mut next_value = 0i128;
        for variant in &item_enum.variants {
            let Some(variant_id) = self.def_id_for_span(variant.span, DefKind::EnumVariant) else {
                continue;
            };
            let value = if let Some(expr) = &variant.value {
                match nia_comptime_engine::eval_int_expr(expr, self) {
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
                .insert(variant_id, ComptimeValue::Int(value));
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

    fn collect_array_lengths_in_ty(&mut self, ty: &TyKind) {
        match ty {
            TyKind::Array { len, elem } => {
                if let ArrayLenTy::ConstExpr { text, span } = len {
                    let value = if let Ok(value) = nia_comptime_engine::eval_array_len_text(text) {
                        Some(value)
                    } else {
                        self.expr_for_span(*span)
                            .cloned()
                            .and_then(|expr| self.eval_array_len_expr(&expr))
                    };
                    if let Some(value) = value {
                        self.array_lengths.insert(*span, value);
                    }
                }
                if let Some(elem) = self.input.interner.get(*elem).cloned() {
                    self.collect_array_lengths_in_ty(&elem);
                }
            }
            TyKind::Pointer { elem, .. } | TyKind::Slice { elem, .. } => {
                if let Some(elem) = self.input.interner.get(*elem).cloned() {
                    self.collect_array_lengths_in_ty(&elem);
                }
            }
            TyKind::FunctionPointer {
                params,
                return_type,
                ..
            } => {
                let params = params.clone();
                for param in params {
                    if let Some(param) = self.input.interner.get(param).cloned() {
                        self.collect_array_lengths_in_ty(&param);
                    }
                }
                if let Some(return_type) = self.input.interner.get(*return_type).cloned() {
                    self.collect_array_lengths_in_ty(&return_type);
                }
            }
            TyKind::Nominal { args, .. } => {
                let args = args.clone();
                for arg in args {
                    if let Some(arg) = self.input.interner.get(arg).cloned() {
                        self.collect_array_lengths_in_ty(&arg);
                    }
                }
            }
            TyKind::Error | TyKind::Primitive(_) | TyKind::GenericParam(_) => {}
        }
    }

    fn eval_array_len_expr(&mut self, expr: &Expr) -> Option<u64> {
        match nia_comptime_engine::eval_array_len_expr(expr, self) {
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
        let result = self.initializer_for_key(key).and_then(|expr| {
            match nia_comptime_engine::eval_expr(&expr, self) {
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

    fn initializer_for_key(&self, key: ComptimeKey) -> Option<Expr> {
        match key {
            ComptimeKey::Global(global_id) => self.global_initializer(global_id),
            ComptimeKey::Local(local_id) => self.local_initializer(local_id),
        }
    }

    fn global_initializer(&self, global_id: GlobalDefId) -> Option<Expr> {
        let (index, defs) = self
            .input
            .all_defs
            .iter()
            .enumerate()
            .find(|(_, defs)| defs.module_id == global_id.module_id)?;
        let module = self.input.all_modules.get(index)?;
        module.items.iter().find_map(|item| {
            let ItemKind::Binding(binding) = &item.kind else {
                return None;
            };
            if !binding.is_comptime {
                return None;
            }
            let def_id = defs.def_spans.get(item.span)?;
            (def_id == global_id.def_id)
                .then(|| binding.value.clone())
                .flatten()
        })
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
                nia_ast::StmtKind::For(for_stmt) => {
                    if let nia_ast::ForHeader::CStyle { init, .. } = &for_stmt.header
                        && let Some(init) = init
                        && let nia_ast::ForInit::Binding { span, binding } = &**init
                        && self.input.locals.local_defs.get(span).copied() == Some(local_id)
                    {
                        return binding.value.clone();
                    }
                    if let Some(value) = self.local_initializer_in_block(local_id, &for_stmt.body) {
                        return Some(value);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn local_comptime_use(&self, span: Span) -> Option<LocalId> {
        let Some(LocalUse::Local(local_id)) = self.input.locals.uses.get(&span) else {
            return None;
        };
        let local = self.input.locals.locals.get(*local_id)?;
        (local.kind == LocalKind::ComptimeBinding).then_some(*local_id)
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
        let defs = self
            .input
            .all_defs
            .iter()
            .find(|defs| defs.module_id == global_id.module_id)?;
        defs.defs.get(global_id.def_id).map(|def| def.kind)
    }

    fn expr_for_span(&self, span: Span) -> Option<&Expr> {
        self.input
            .module
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Binding(binding) => binding
                    .ty
                    .as_ref()
                    .and_then(|ty| expr_for_span_in_type(ty, span))
                    .or_else(|| {
                        binding
                            .value
                            .as_ref()
                            .and_then(|value| expr_for_span(value, span))
                    }),
                ItemKind::Function(function) => {
                    for param in &function.params {
                        if let Some(ty) = &param.ty
                            && let Some(expr) = expr_for_span_in_type(ty, span)
                        {
                            return Some(expr);
                        }
                    }
                    if let Some(ty) = &function.return_type
                        && let Some(expr) = expr_for_span_in_type(ty, span)
                    {
                        return Some(expr);
                    }
                    function
                        .body
                        .as_ref()
                        .and_then(|body| expr_for_span_in_block(body, span))
                }
                ItemKind::Extend(extend) => extend.methods.iter().find_map(|method| {
                    method.function.body.as_ref().and_then(|body| {
                        for param in &method.function.params {
                            if let Some(ty) = &param.ty
                                && let Some(expr) = expr_for_span_in_type(ty, span)
                            {
                                return Some(expr);
                            }
                        }
                        if let Some(ty) = &method.function.return_type
                            && let Some(expr) = expr_for_span_in_type(ty, span)
                        {
                            return Some(expr);
                        }
                        expr_for_span_in_block(body, span)
                    })
                }),
                ItemKind::Struct(item_struct) => item_struct
                    .fields
                    .iter()
                    .find_map(|field| expr_for_span_in_type(&field.ty, span)),
                ItemKind::Union(item_union) => item_union
                    .fields
                    .iter()
                    .find_map(|field| expr_for_span_in_type(&field.ty, span)),
                ItemKind::TypeAlias(alias) => expr_for_span_in_type(&alias.ty, span),
                ItemKind::Enum(item_enum) => item_enum.variants.iter().find_map(|variant| {
                    variant
                        .value
                        .as_ref()
                        .and_then(|value| expr_for_span(value, span))
                }),
                ItemKind::Import(_) | ItemKind::Using(_) => None,
            })
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

impl ComptimeEnv for Analyzer<'_> {
    fn resolve_ident(&mut self, span: Span, name: &str) -> Result<ComptimeValue, ComptimeError> {
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
}

fn expr_for_span(expr: &Expr, span: Span) -> Option<&Expr> {
    if expr.span == span {
        return Some(expr);
    }
    match &expr.kind {
        ExprKind::Unary { expr, .. } | ExprKind::Cast { expr, .. } => expr_for_span(expr, span),
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::Assign { lhs, rhs, .. } => {
            expr_for_span(lhs, span).or_else(|| expr_for_span(rhs, span))
        }
        ExprKind::Call { callee, args } => expr_for_span(callee, span)
            .or_else(|| args.iter().find_map(|arg| expr_for_span(arg, span))),
        ExprKind::ArrayLiteral { elems } => match elems {
            nia_ast::ArrayElements::List(elems) => {
                elems.iter().find_map(|elem| expr_for_span(elem, span))
            }
            nia_ast::ArrayElements::Repeat { value, count } => {
                expr_for_span(value, span).or_else(|| expr_for_span(count, span))
            }
        },
        ExprKind::StructLiteral { fields } => fields
            .iter()
            .find_map(|field| expr_for_span(&field.value, span)),
        ExprKind::Field { lhs, .. } | ExprKind::Qualified { lhs, .. } => expr_for_span(lhs, span),
        ExprKind::Index { lhs, index } => expr_for_span(lhs, span).or_else(|| match index {
            nia_ast::IndexArg::Expr(index) => expr_for_span(index, span),
            nia_ast::IndexArg::Range(range) => range
                .start
                .as_ref()
                .and_then(|start| expr_for_span(start, span))
                .or_else(|| range.end.as_ref().and_then(|end| expr_for_span(end, span))),
        }),
        ExprKind::Block(block) => expr_for_span_in_block(block, span),
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => expr_for_span(cond, span)
            .or_else(|| expr_for_span_in_block(then_branch, span))
            .or_else(|| {
                else_branch
                    .as_ref()
                    .and_then(|expr| expr_for_span(expr, span))
            }),
        ExprKind::Switch(switch) => expr_for_span(&switch.target, span).or_else(|| {
            switch.arms.iter().find_map(|arm| {
                let pattern = match &arm.pattern {
                    nia_ast::SwitchPattern::Expr(pattern) => expr_for_span(pattern, span),
                    nia_ast::SwitchPattern::Default => None,
                };
                pattern.or_else(|| match &arm.body {
                    nia_ast::SwitchArmBody::Expr(expr) => expr_for_span(expr, span),
                    nia_ast::SwitchArmBody::Stmt(stmt) => expr_for_span_in_stmt(stmt, span),
                    nia_ast::SwitchArmBody::Block(block) => expr_for_span_in_block(block, span),
                })
            })
        }),
        ExprKind::BracketSuffix { callee, args } => expr_for_span(callee, span).or_else(|| {
            args.iter()
                .filter_map(|arg| arg.expr.as_ref())
                .find_map(|expr| expr_for_span(expr, span))
        }),
        ExprKind::Error
        | ExprKind::Integer(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::ByteString(_)
        | ExprKind::CString(_)
        | ExprKind::Char(_)
        | ExprKind::ByteChar(_)
        | ExprKind::Bool(_)
        | ExprKind::Ident(_)
        | ExprKind::Builtin { .. }
        | ExprKind::TypeTarget { .. }
        | ExprKind::Underscore
        | ExprKind::Raw(_) => None,
    }
}

fn expr_for_span_in_block(block: &nia_ast::Block, span: Span) -> Option<&Expr> {
    for stmt in &block.stmts {
        let found = match &stmt.kind {
            nia_ast::StmtKind::Binding(binding) => binding
                .ty
                .as_ref()
                .and_then(|ty| expr_for_span_in_type(ty, span))
                .or_else(|| {
                    binding
                        .value
                        .as_ref()
                        .and_then(|value| expr_for_span(value, span))
                }),
            nia_ast::StmtKind::Expr(expr)
            | nia_ast::StmtKind::Return(Some(expr))
            | nia_ast::StmtKind::Defer(expr) => expr_for_span(expr, span),
            nia_ast::StmtKind::For(for_stmt) => match &for_stmt.header {
                nia_ast::ForHeader::Infinite => expr_for_span_in_block(&for_stmt.body, span),
                nia_ast::ForHeader::Condition(cond) => expr_for_span(cond, span)
                    .or_else(|| expr_for_span_in_block(&for_stmt.body, span)),
                nia_ast::ForHeader::CStyle { init, cond, step } => init
                    .as_ref()
                    .and_then(|init| match &**init {
                        nia_ast::ForInit::Binding { binding, .. } => binding
                            .ty
                            .as_ref()
                            .and_then(|ty| expr_for_span_in_type(ty, span))
                            .or_else(|| {
                                binding
                                    .value
                                    .as_ref()
                                    .and_then(|value| expr_for_span(value, span))
                            }),
                        nia_ast::ForInit::Expr(expr) => expr_for_span(expr, span),
                    })
                    .or_else(|| cond.as_ref().and_then(|cond| expr_for_span(cond, span)))
                    .or_else(|| step.as_ref().and_then(|step| expr_for_span(step, span)))
                    .or_else(|| expr_for_span_in_block(&for_stmt.body, span)),
            },
            nia_ast::StmtKind::Using(_)
            | nia_ast::StmtKind::Return(None)
            | nia_ast::StmtKind::Break
            | nia_ast::StmtKind::Continue => None,
        };
        if found.is_some() {
            return found;
        }
    }
    block
        .tail
        .as_ref()
        .and_then(|tail| expr_for_span(tail, span))
}

fn expr_for_span_in_stmt(stmt: &nia_ast::Stmt, span: Span) -> Option<&Expr> {
    match &stmt.kind {
        nia_ast::StmtKind::Binding(binding) => binding
            .ty
            .as_ref()
            .and_then(|ty| expr_for_span_in_type(ty, span))
            .or_else(|| {
                binding
                    .value
                    .as_ref()
                    .and_then(|value| expr_for_span(value, span))
            }),
        nia_ast::StmtKind::Expr(expr)
        | nia_ast::StmtKind::Return(Some(expr))
        | nia_ast::StmtKind::Defer(expr) => expr_for_span(expr, span),
        nia_ast::StmtKind::For(for_stmt) => expr_for_span_in_block(&for_stmt.body, span),
        nia_ast::StmtKind::Using(_)
        | nia_ast::StmtKind::Return(None)
        | nia_ast::StmtKind::Break
        | nia_ast::StmtKind::Continue => None,
    }
}

fn expr_for_span_in_type(ty: &nia_ast::TypeRef, span: Span) -> Option<&Expr> {
    match &ty.kind {
        nia_ast::TypeKind::Array { len, elem } => {
            if let nia_ast::ArrayLen::Expr(expr) = len
                && (expr.span == span || expr_for_span(expr, span).is_some())
            {
                return expr_for_span(expr, span).or(Some(expr));
            }
            expr_for_span_in_type(elem, span)
        }
        nia_ast::TypeKind::Pointer { elem, .. } | nia_ast::TypeKind::Slice { elem, .. } => {
            expr_for_span_in_type(elem, span)
        }
        nia_ast::TypeKind::FunctionPointer {
            params,
            return_type,
            ..
        } => params
            .iter()
            .find_map(|param| expr_for_span_in_type(param, span))
            .or_else(|| {
                return_type
                    .as_ref()
                    .and_then(|return_type| expr_for_span_in_type(return_type, span))
            }),
        nia_ast::TypeKind::Path { segments } => segments.iter().find_map(|segment| {
            segment.args.iter().find_map(|arg| match arg {
                nia_ast::TypeArg::Type(ty) => expr_for_span_in_type(ty, span),
                nia_ast::TypeArg::Const(_) => None,
            })
        }),
        nia_ast::TypeKind::Error
        | nia_ast::TypeKind::Void
        | nia_ast::TypeKind::Never
        | nia_ast::TypeKind::Infer => None,
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
