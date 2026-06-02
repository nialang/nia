// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_ast::{Expr, ItemKind, Module, TypeRef};
use nia_comptime_engine::{ComptimeEnv, ComptimeError};
use nia_defs::{DefCollection, DefId, DefKind};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalConstExprId, GlobalDefId, LayoutBuiltin, LocalId, ModuleId};
use nia_item_signatures::ItemSignatures;
use nia_local_resolve::{LocalKind, LocalResolution, LocalUse};
use nia_span::Span;
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
    pub module: &'a Module,
    pub defs: &'a DefCollection,
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub signatures: &'a ItemSignatures,
    pub interner: &'a TyInterner,
    pub type_uses: &'a HashMap<Span, nia_ids::InternedTyId>,
    pub normalized: &'a HashMap<nia_ids::InternedTyId, nia_ids::InternedTyId>,
    pub const_exprs: &'a HashMap<GlobalConstExprId, Expr>,
    pub program: ComptimeProgramContext<'a>,
}

#[derive(Debug, Clone, Copy)]
pub struct ComptimeProgramContext<'a> {
    pub modules: Option<&'a HashMap<ModuleId, Module>>,
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
    array_lengths: HashMap<GlobalConstExprId, u64>,
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
        for (id, expr) in self.input.const_exprs {
            let expr = expr.clone();
            if let Some(value) = self.eval_array_len_expr(&expr) {
                self.array_lengths.insert(*id, value);
            }
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
        let (module, defs) = if global_id.module_id == self.input.defs.module_id {
            (self.input.module, self.input.defs)
        } else {
            (
                self.input.program.modules?.get(&global_id.module_id)?,
                self.input.program.defs?.get(&global_id.module_id)?,
            )
        };
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

    fn global_defs(&self, module_id: ModuleId) -> Option<&DefCollection> {
        if module_id == self.input.defs.module_id {
            Some(self.input.defs)
        } else {
            self.input.program.defs?.get(&module_id)
        }
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
        self.global_defs(global_id.module_id)?
            .defs
            .get(global_id.def_id)
            .map(|def| def.kind)
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

    fn resolve_layout_builtin(
        &mut self,
        span: Span,
        builtin: LayoutBuiltin,
        ty: &TypeRef,
    ) -> Result<ComptimeValue, ComptimeError> {
        let Some(ty_id) = self.ty_for_span(ty.span) else {
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
