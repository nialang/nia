// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_ast::{Expr, ItemKind, Module};
use nia_comptime_engine::{ComptimeCommonEnv, ComptimeError, ResolvedComptimeEnv};
use nia_comptime_ir::{
    ComptimeBinaryOp, ComptimeNameResolution, ComptimeStringLiteral, ComptimeUnaryOp,
    ResolvedComptimeArrayElements, ResolvedComptimeArrayElementsKind, ResolvedComptimeAssignTarget,
    ResolvedComptimeAssignTargetKind, ResolvedComptimeBinding, ResolvedComptimeBlock,
    ResolvedComptimeEnum, ResolvedComptimeEnumVariant, ResolvedComptimeExpr,
    ResolvedComptimeExprKind, ResolvedComptimeFieldInit, ResolvedComptimeLocalInitializer,
    ResolvedComptimeModule, ResolvedComptimeParam, ResolvedComptimeStmtKind,
    ResolvedComptimeSwitch, ResolvedComptimeSwitchArmBody, ResolvedComptimeSwitchArmBodyKind,
    ResolvedComptimeSwitchPattern, ResolvedComptimeSwitchPatternKind, ResolvedComptimeTypeArg,
};
use nia_defs::{DefCollection, DefId, DefKind};
use nia_diagnostic::Diagnostic;
use nia_ids::{
    GlobalConstExprId, GlobalDefId, InternedTyId, LayoutBuiltin, LocalId, ModuleId, ValueBuiltin,
};
use nia_item_signatures::{FunctionSignature, ItemSignatures};
use nia_local_resolve::{LocalKind, LocalResolution, LocalUse};
use nia_sema::{
    ArityCheck, ArrayLiteralLenCheck, FieldSetCheck, NamedField, check_array_literal_len,
    check_exact_arity, check_required_field_set, check_value_field_set,
};
use nia_span::Span;
use nia_target_config::TargetConfig;
use nia_ty::{ArrayLenTy, PrimitiveTy, RangeTyKind, TyInterner, TyKind, import_type_into};
use nia_value_resolve::{ValueNameResolution, ValueResolution};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ComptimeCheck {
    pub values: HashMap<ComptimeKey, ComptimeValue>,
    pub typed_values: HashMap<ComptimeKey, TypedComptimeValue>,
    pub enum_values: HashMap<DefId, ComptimeValue>,
    pub typed_enum_values: HashMap<DefId, TypedComptimeValue>,
    pub array_lengths: HashMap<GlobalConstExprId, u64>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedComptimeValue {
    pub value: ComptimeValue,
    pub ty: ComptimeValueType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComptimeValueType {
    Runtime(InternedTyId),
    Int,
    Bool,
    String,
    Array {
        elem: Box<ComptimeValueType>,
        len: Option<u64>,
    },
    Struct(Vec<ComptimeValueFieldType>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComptimeValueFieldType {
    pub name: String,
    pub ty: ComptimeValueType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ComptimeArmType {
    Value(ComptimeValueType),
    ControlFlow,
}

impl ComptimeValueType {
    pub fn runtime(&self) -> Option<InternedTyId> {
        match self {
            Self::Runtime(ty) => Some(*ty),
            Self::Int | Self::Bool | Self::String | Self::Array { .. } | Self::Struct(_) => None,
        }
    }

    pub fn structural_field(&self, name: &str) -> Option<&ComptimeValueType> {
        let Self::Struct(fields) = self else {
            return None;
        };
        fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| &field.ty)
    }

    pub fn array_elem(&self) -> Option<(&ComptimeValueType, Option<u64>)> {
        let Self::Array { elem, len } = self else {
            return None;
        };
        Some((elem, *len))
    }
}

fn resolved_switch_pattern_local_id(pattern: &ResolvedComptimeSwitchPattern) -> Option<LocalId> {
    match pattern.kind() {
        ResolvedComptimeSwitchPatternKind::OptionalSome { local_id, .. }
        | ResolvedComptimeSwitchPatternKind::ErrorOk { local_id, .. }
        | ResolvedComptimeSwitchPatternKind::ErrorErr { local_id, .. } => Some(*local_id),
        ResolvedComptimeSwitchPatternKind::Default
        | ResolvedComptimeSwitchPatternKind::OptionalNull { .. }
        | ResolvedComptimeSwitchPatternKind::Expr(_)
        | ResolvedComptimeSwitchPatternKind::Range { .. } => None,
    }
}

pub fn import_comptime_value_type(
    source: &TyInterner,
    target: &mut TyInterner,
    ty: ComptimeValueType,
) -> Option<ComptimeValueType> {
    match ty {
        ComptimeValueType::Runtime(ty) => Some(ComptimeValueType::Runtime(import_type_into(
            target, source, ty,
        ))),
        ComptimeValueType::Array { elem, len } => Some(ComptimeValueType::Array {
            elem: Box::new(import_comptime_value_type(source, target, *elem)?),
            len,
        }),
        ComptimeValueType::Struct(fields) => fields
            .into_iter()
            .map(|field| {
                Some(ComptimeValueFieldType {
                    name: field.name,
                    ty: import_comptime_value_type(source, target, field.ty)?,
                })
            })
            .collect::<Option<Vec<_>>>()
            .map(ComptimeValueType::Struct),
        ComptimeValueType::Int => Some(ComptimeValueType::Int),
        ComptimeValueType::Bool => Some(ComptimeValueType::Bool),
        ComptimeValueType::String => Some(ComptimeValueType::String),
    }
}

pub fn builtin_comptime_value_type(pointer_width_ty: InternedTyId) -> ComptimeValueType {
    ComptimeValueType::Struct(vec![ComptimeValueFieldType {
        name: "target".to_string(),
        ty: ComptimeValueType::Struct(vec![
            ComptimeValueFieldType {
                name: "arch".to_string(),
                ty: ComptimeValueType::String,
            },
            ComptimeValueFieldType {
                name: "vendor".to_string(),
                ty: ComptimeValueType::String,
            },
            ComptimeValueFieldType {
                name: "os".to_string(),
                ty: ComptimeValueType::String,
            },
            ComptimeValueFieldType {
                name: "env".to_string(),
                ty: ComptimeValueType::String,
            },
            ComptimeValueFieldType {
                name: "abi".to_string(),
                ty: ComptimeValueType::String,
            },
            ComptimeValueFieldType {
                name: "endian".to_string(),
                ty: ComptimeValueType::String,
            },
            ComptimeValueFieldType {
                name: "pointer_width".to_string(),
                ty: ComptimeValueType::Runtime(pointer_width_ty),
            },
        ]),
    }])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComptimeKey {
    Global(GlobalDefId),
    Local(LocalId),
}

pub use nia_comptime_engine::ComptimeValue;

#[derive(Debug, Clone, Copy)]
pub struct ComptimeInput<'a> {
    pub module: &'a ResolvedComptimeModule,
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
    pub module: ResolvedComptimeModule,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy)]
pub struct ComptimeModuleInput<'a> {
    pub module: &'a Module,
    pub defs: &'a DefCollection,
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub type_uses: &'a HashMap<Span, InternedTyId>,
    pub const_exprs: &'a HashMap<GlobalConstExprId, Expr>,
}

#[derive(Debug, Clone, Copy)]
pub struct ComptimeProgramContext<'a> {
    pub modules: Option<&'a HashMap<ModuleId, ResolvedComptimeModule>>,
    pub defs: Option<&'a HashMap<ModuleId, DefCollection>>,
    pub type_lowerings: Option<&'a HashMap<ModuleId, nia_type_lower::TypeLowering>>,
    pub type_normalizations: Option<&'a HashMap<ModuleId, nia_type_normalize::TypeNormalization>>,
    pub signatures: Option<&'a HashMap<ModuleId, ItemSignatures>>,
}

impl<'a> ComptimeProgramContext<'a> {
    pub fn empty() -> Self {
        Self {
            modules: None,
            defs: None,
            type_lowerings: None,
            type_normalizations: None,
            signatures: None,
        }
    }
}

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

#[derive(Debug, Clone, Default)]
pub struct TypedComptimeFrame {
    pub module_id: Option<ModuleId>,
    pub local_types: HashMap<LocalId, ComptimeValueType>,
    pub type_substitutions: HashMap<String, InternedTyId>,
}

#[derive(Debug, Clone, Copy)]
pub struct TypedComptimeQueryInput<'a> {
    pub module: &'a ResolvedComptimeModule,
    pub defs: &'a DefCollection,
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub signatures: &'a ItemSignatures,
    pub interner: &'a TyInterner,
    pub type_uses: &'a HashMap<Span, InternedTyId>,
    pub normalized: &'a HashMap<InternedTyId, InternedTyId>,
    pub target: &'a TargetConfig,
    pub program: ComptimeProgramContext<'a>,
    pub typed_values: &'a HashMap<ComptimeKey, TypedComptimeValue>,
    pub array_lengths: &'a HashMap<GlobalConstExprId, u64>,
    pub frames: &'a [TypedComptimeFrame],
}

pub fn instantiate_resolved_comptime_function_generics(
    input: TypedComptimeQueryInput<'_>,
    span: Span,
    signature_module_id: ModuleId,
    signature: &FunctionSignature,
    type_args: &[ResolvedComptimeTypeArg],
    arg_exprs: &[ResolvedComptimeExpr],
) -> Result<HashMap<String, InternedTyId>, ComptimeError> {
    let mut analyzer = Analyzer::for_typed_query(input);
    analyzer.instantiate_resolved_function_generics(
        span,
        signature_module_id,
        signature,
        type_args,
        arg_exprs,
    )
}

pub fn infer_resolved_comptime_expr_type(
    input: TypedComptimeQueryInput<'_>,
    expr: &ResolvedComptimeExpr,
    expected: Option<InternedTyId>,
) -> Option<ComptimeValueType> {
    let mut analyzer = Analyzer::for_typed_query(input);
    analyzer.resolved_comptime_expr_type(expr, expected)
}

pub fn check_module_comptime(input: ComptimeInput<'_>) -> ComptimeCheck {
    let mut analyzer = Analyzer {
        input,
        values: HashMap::new(),
        typed_values: HashMap::new(),
        external_typed_values: None,
        call_locals: Vec::new(),
        execution_module_overrides: Vec::new(),
        enum_values: HashMap::new(),
        typed_enum_values: HashMap::new(),
        array_lengths: HashMap::new(),
        diagnostics: Vec::new(),
        active: HashSet::new(),
        working_interners: HashMap::from([(input.defs.module_id, input.interner.clone())]),
    };
    analyzer.analyze_module();
    ComptimeCheck {
        values: analyzer.values,
        typed_values: analyzer.typed_values,
        enum_values: analyzer.enum_values,
        typed_enum_values: analyzer.typed_enum_values,
        array_lengths: analyzer.array_lengths,
        diagnostics: analyzer.diagnostics,
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
                Some(ResolvedComptimeEnumVariant::new(
                    self.global_def_id(variant_id),
                    variant.span,
                    value,
                ))
            })
            .collect();
        self.module.push_enum(ResolvedComptimeEnum::new(
            self.global_def_id(enum_id),
            item_span,
            variants,
        ));
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
                .insert_global_initializer(self.global_def_id(def_id), value);
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
        let type_id = |span| self.input.type_uses.get(&span).copied();
        let context = nia_comptime_ir::ResolvedComptimeLowerInputs::new(
            &name_resolution,
            &local_id,
            &type_id,
        );
        match nia_comptime_ir::lower_function_resolved_with_context(
            function_span,
            function,
            &context,
        ) {
            Ok(function) => {
                self.module
                    .insert_function(self.global_def_id(def_id), function);
            }
            Err(err) => self
                .diagnostics
                .push(Diagnostic::error(err.span, err.message)),
        }
    }

    fn lower_expr(&mut self, expr: &Expr) -> Option<ResolvedComptimeExpr> {
        let mut allowed_locals = HashSet::new();
        self.collect_expr_locals(expr, &mut allowed_locals);
        let name_resolution =
            |span| self.name_resolution_with_allowed_locals(span, &allowed_locals);
        let local_id = |span| self.input.locals.local_defs.get(&span).copied();
        let type_id = |span| self.input.type_uses.get(&span).copied();
        let context = nia_comptime_ir::ResolvedComptimeLowerInputs::new(
            &name_resolution,
            &local_id,
            &type_id,
        );
        match nia_comptime_ir::lower_expr_resolved_with_context(expr, &context) {
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
        if let Some(local_id) = self.local_use(span)
            && allowed_locals.contains(&local_id)
        {
            return Some(ComptimeNameResolution::Local(local_id));
        }
        if let Some(local_id) = self.local_comptime_use(span) {
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
            nia_ast::SwitchPattern::OptionalSome { span, .. }
            | nia_ast::SwitchPattern::ErrorOk { span, .. }
            | nia_ast::SwitchPattern::ErrorErr { span, .. } => {
                if let Some(local_id) = self.input.locals.local_defs.get(span).copied() {
                    out.insert(local_id);
                }
            }
            nia_ast::SwitchPattern::Expr(expr) => self.collect_expr_locals(expr, out),
            nia_ast::SwitchPattern::Range { start, end, .. } => {
                self.collect_expr_locals(start, out);
                self.collect_expr_locals(end, out);
            }
            nia_ast::SwitchPattern::Default | nia_ast::SwitchPattern::OptionalNull { .. } => {}
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
                    if self.input.locals.local_defs.get(&stmt.span).copied() == Some(local_id) =>
                {
                    return binding.value.clone().map(|value| {
                        (
                            value,
                            binding
                                .ty
                                .as_ref()
                                .and_then(|ty| self.input.type_uses.get(&ty.span).copied()),
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
    typed_values: HashMap<ComptimeKey, TypedComptimeValue>,
    external_typed_values: Option<&'a HashMap<ComptimeKey, TypedComptimeValue>>,
    call_locals: Vec<ComptimeCallFrame>,
    execution_module_overrides: Vec<ModuleId>,
    enum_values: HashMap<DefId, ComptimeValue>,
    typed_enum_values: HashMap<DefId, TypedComptimeValue>,
    array_lengths: HashMap<GlobalConstExprId, u64>,
    diagnostics: Vec<Diagnostic>,
    active: HashSet<ComptimeKey>,
    working_interners: HashMap<ModuleId, TyInterner>,
}

#[derive(Debug, Clone, Default)]
struct ComptimeCallFrame {
    module_id: Option<ModuleId>,
    locals: HashMap<LocalId, ComptimeValue>,
    local_types: HashMap<LocalId, ComptimeValueType>,
    mutable_locals: HashSet<LocalId>,
    type_substitutions: HashMap<String, InternedTyId>,
}

impl From<TypedComptimeFrame> for ComptimeCallFrame {
    fn from(frame: TypedComptimeFrame) -> Self {
        Self {
            module_id: frame.module_id,
            locals: HashMap::new(),
            local_types: frame.local_types,
            mutable_locals: HashSet::new(),
            type_substitutions: frame.type_substitutions,
        }
    }
}

impl Analyzer<'_> {
    fn for_typed_query(input: TypedComptimeQueryInput<'_>) -> Analyzer<'_> {
        Analyzer {
            input: ComptimeInput {
                module: input.module,
                defs: input.defs,
                values: input.values,
                locals: input.locals,
                signatures: input.signatures,
                interner: input.interner,
                type_uses: input.type_uses,
                normalized: input.normalized,
                target: input.target,
                program: input.program,
            },
            values: HashMap::new(),
            typed_values: HashMap::new(),
            external_typed_values: Some(input.typed_values),
            call_locals: input
                .frames
                .iter()
                .cloned()
                .map(ComptimeCallFrame::from)
                .collect(),
            execution_module_overrides: Vec::new(),
            enum_values: HashMap::new(),
            typed_enum_values: HashMap::new(),
            array_lengths: input.array_lengths.clone(),
            diagnostics: Vec::new(),
            active: HashSet::new(),
            working_interners: HashMap::from([(
                input.interner.interner_id(),
                input.interner.clone(),
            )]),
        }
    }

    fn analyze_module(&mut self) {
        let enums = self.input.module.enums().to_vec();
        for item_enum in &enums {
            self.eval_enum(item_enum);
        }
        let const_exprs = self.input.module.const_exprs().clone();
        for (id, expr) in const_exprs {
            if let Some(value) = self.eval_resolved_array_len_expr(&expr) {
                self.array_lengths.insert(id, value);
            }
        }
        let global_initializers = self
            .input
            .module
            .global_initializers()
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
            .local_initializers()
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
    }

    fn eval_enum(&mut self, item_enum: &ResolvedComptimeEnum) {
        let enum_id = item_enum.def_id();
        let range = self.enum_backing_range(enum_id.def_id);
        let mut next_value = 0i128;
        for variant in item_enum.variants() {
            let value = if let Some(expr) = variant.value() {
                match nia_comptime_engine::eval_resolved_comptime_int_expr(expr, self) {
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
                    variant.span(),
                    format!("enum variant value {value} is out of range for backing type"),
                ));
            }
            let variant_id = variant.def_id();
            self.enum_values
                .insert(variant_id.def_id, ComptimeValue::Int(value));
            let ty = self
                .input
                .signatures
                .enums
                .get(&enum_id.def_id)
                .map(|signature| signature.backing_type)
                .unwrap_or_else(|| self.input.interner.primitive(PrimitiveTy::Isize));
            self.typed_enum_values.insert(
                variant_id.def_id,
                TypedComptimeValue {
                    value: ComptimeValue::Int(value),
                    ty: ComptimeValueType::Runtime(ty),
                },
            );
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

    fn eval_resolved_array_len_expr(&mut self, expr: &ResolvedComptimeExpr) -> Option<u64> {
        match nia_comptime_engine::eval_resolved_comptime_array_len_expr(expr, self) {
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
        let module_id = self.key_module_id(key);
        let result = self.initializer_for_key(key).cloned().and_then(|expr| {
            self.with_execution_module(module_id, |this| {
                match nia_comptime_engine::eval_resolved_comptime_expr(&expr, this) {
                    Ok(value) => Some(value),
                    Err(err) => {
                        this.push_engine_error(err);
                        None
                    }
                }
            })
        });
        self.active.remove(&key);
        result.map(|value| {
            let value = self.insert_typed_key_value(key, value);
            self.values.insert(key, value.clone());
            value
        })
    }

    fn insert_typed_key_value(&mut self, key: ComptimeKey, value: ComptimeValue) -> ComptimeValue {
        let module_id = self.key_module_id(key);
        self.with_execution_module(module_id, |this| {
            let Some(ty) = this.comptime_value_type_for_key(key) else {
                return value;
            };
            let value = this.normalize_typed_comptime_value(value, &ty);
            if let Some(span) = this.initializer_span_for_key(key) {
                this.validate_typed_value(span, &value, &ty);
            }
            this.typed_values.insert(
                key,
                TypedComptimeValue {
                    value: value.clone(),
                    ty,
                },
            );
            value
        })
    }

    fn typed_value_for_key(&self, key: ComptimeKey) -> Option<&TypedComptimeValue> {
        self.typed_values.get(&key).or_else(|| {
            self.external_typed_values
                .and_then(|values| values.get(&key))
        })
    }

    fn comptime_value_type_for_key(&mut self, key: ComptimeKey) -> Option<ComptimeValueType> {
        self.explicit_type_for_key(key)
            .map(ComptimeValueType::Runtime)
            .or_else(|| self.inferred_type_for_key(key))
    }

    fn inferred_type_for_key(&mut self, key: ComptimeKey) -> Option<ComptimeValueType> {
        let expr = self.initializer_for_key(key)?.clone();
        let module_id = self.key_module_id(key);
        self.with_execution_module(module_id, |this| {
            this.resolved_comptime_expr_type(&expr, None)
        })
    }

    fn key_module_id(&self, key: ComptimeKey) -> ModuleId {
        match key {
            ComptimeKey::Global(global_id) => global_id.module_id,
            ComptimeKey::Local(_) => self.input.defs.module_id,
        }
    }

    fn initializer_span_for_key(&self, key: ComptimeKey) -> Option<Span> {
        self.initializer_for_key(key).map(|expr| expr.span())
    }

    fn validate_typed_value(&mut self, span: Span, value: &ComptimeValue, ty: &ComptimeValueType) {
        match ty {
            ComptimeValueType::Runtime(ty) => self.validate_runtime_typed_value(span, value, *ty),
            ComptimeValueType::Array { elem, .. } => {
                let ComptimeValue::Array(values) = value else {
                    self.push_comptime_type_mismatch(span, "array");
                    return;
                };
                if let ComptimeValueType::Array { len: Some(len), .. } = ty
                    && values.len() as u64 != *len
                {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!(
                            "comptime array length {} does not match expected length {len}",
                            values.len()
                        ),
                    ));
                }
                for value in values {
                    self.validate_typed_value(span, value, elem);
                }
            }
            ComptimeValueType::Struct(fields) => {
                let ComptimeValue::Struct(values) = value else {
                    self.push_comptime_type_mismatch(span, "struct");
                    return;
                };
                let field_set: FieldSetCheck<String> = check_value_field_set(
                    values.keys().cloned(),
                    fields.iter().map(|field| field.name.clone()),
                );
                for field in fields {
                    if let Some(value) = values.get(&field.name) {
                        self.validate_typed_value(span, value, &field.ty);
                    }
                }
                for name in &field_set.missing_fields {
                    self.push_comptime_missing_struct_field(span, name);
                }
                for field in &field_set.unknown_fields {
                    self.push_comptime_extra_struct_field(span, &field.name);
                }
            }
            ComptimeValueType::Int => {
                if !matches!(value, ComptimeValue::Int(_)) {
                    self.push_comptime_type_mismatch(span, "int");
                }
            }
            ComptimeValueType::Bool => {
                if !matches!(value, ComptimeValue::Bool(_)) {
                    self.push_comptime_type_mismatch(span, "bool");
                }
            }
            ComptimeValueType::String => {
                if !matches!(value, ComptimeValue::String(_)) {
                    self.push_comptime_type_mismatch(span, "string");
                }
            }
        }
    }

    fn normalize_typed_comptime_value(
        &mut self,
        value: ComptimeValue,
        ty: &ComptimeValueType,
    ) -> ComptimeValue {
        match ty {
            ComptimeValueType::Runtime(ty) => {
                self.normalize_runtime_typed_comptime_value(value, *ty)
            }
            ComptimeValueType::Array { elem, .. } => match value {
                ComptimeValue::Array(values) => ComptimeValue::Array(
                    values
                        .into_iter()
                        .map(|value| self.normalize_typed_comptime_value(value, elem))
                        .collect(),
                ),
                value => value,
            },
            ComptimeValueType::Struct(fields) => match value {
                ComptimeValue::Struct(mut values) => {
                    for field in fields {
                        if let Some(value) = values.remove(&field.name) {
                            values.insert(
                                field.name.clone(),
                                self.normalize_typed_comptime_value(value, &field.ty),
                            );
                        }
                    }
                    ComptimeValue::Struct(values)
                }
                value => value,
            },
            ComptimeValueType::Int | ComptimeValueType::Bool | ComptimeValueType::String => value,
        }
    }

    fn normalize_runtime_typed_comptime_value(
        &mut self,
        value: ComptimeValue,
        ty: InternedTyId,
    ) -> ComptimeValue {
        match self.ty_kind(ty) {
            Some(TyKind::Array { elem, .. }) => {
                if self.runtime_array_accepts_comptime_string(&value, ty)
                    && let ComptimeValue::String(value) = value
                {
                    return ComptimeValue::Array(comptime_string_to_char_array(&value));
                }
                match value {
                    ComptimeValue::Array(values) => ComptimeValue::Array(
                        values
                            .into_iter()
                            .map(|value| self.normalize_runtime_typed_comptime_value(value, elem))
                            .collect(),
                    ),
                    value => value,
                }
            }
            Some(TyKind::Optional { elem }) => match value {
                ComptimeValue::Optional(Some(value)) => ComptimeValue::Optional(Some(Box::new(
                    self.normalize_runtime_typed_comptime_value(*value, elem),
                ))),
                value => value,
            },
            Some(TyKind::ErrorUnion { error, value: ok }) => match value {
                ComptimeValue::ErrorUnion(Ok(value)) => ComptimeValue::ErrorUnion(Ok(Box::new(
                    self.normalize_runtime_typed_comptime_value(*value, ok),
                ))),
                ComptimeValue::ErrorUnion(Err(value)) => ComptimeValue::ErrorUnion(Err(Box::new(
                    self.normalize_runtime_typed_comptime_value(*value, error),
                ))),
                value => value,
            },
            Some(TyKind::Nominal { .. }) => self.normalize_nominal_struct_value(value, ty),
            _ => value,
        }
    }

    fn normalize_nominal_struct_value(
        &mut self,
        value: ComptimeValue,
        ty: InternedTyId,
    ) -> ComptimeValue {
        let ComptimeValue::Struct(mut values) = value else {
            return value;
        };
        let Some((def_id, args)) = self.expected_nominal_parts(ty) else {
            return ComptimeValue::Struct(values);
        };
        if self.def_kind_of(def_id) != Some(DefKind::Struct) {
            return ComptimeValue::Struct(values);
        }
        let Some(signature) = self.struct_signature_for(def_id) else {
            return ComptimeValue::Struct(values);
        };
        let Some(field_tys) = self.comptime_struct_field_types(&signature, &args) else {
            return ComptimeValue::Struct(values);
        };
        for (name, ty) in field_tys {
            if let Some(value) = values.remove(&name) {
                values.insert(name, self.normalize_runtime_typed_comptime_value(value, ty));
            }
        }
        ComptimeValue::Struct(values)
    }

    fn validate_runtime_typed_value(
        &mut self,
        span: Span,
        value: &ComptimeValue,
        ty: InternedTyId,
    ) {
        match self.ty_kind(ty) {
            Some(TyKind::Primitive(primitive)) => {
                self.validate_primitive_typed_value(span, value, primitive)
            }
            Some(TyKind::Array { elem, .. }) => {
                if self.runtime_array_accepts_comptime_string(value, ty) {
                    return;
                }
                if let ComptimeValue::String(value) = value
                    && self.runtime_array_is_char_array(ty)
                    && let Some(expected_len) = self.runtime_array_len(ty)
                {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!(
                            "comptime array length {} does not match expected length {expected_len}",
                            value.chars().count()
                        ),
                    ));
                    return;
                }
                let ComptimeValue::Array(values) = value else {
                    self.push_comptime_type_mismatch(span, "array");
                    return;
                };
                if let Some(expected_len) = self.runtime_array_len(ty)
                    && values.len() as u64 != expected_len
                {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!(
                            "comptime array length {} does not match expected length {expected_len}",
                            values.len()
                        ),
                    ));
                }
                for value in values {
                    self.validate_runtime_typed_value(span, value, elem);
                }
            }
            Some(TyKind::Optional { elem }) => match value {
                ComptimeValue::Optional(Some(value)) => {
                    self.validate_runtime_typed_value(span, value, elem);
                }
                ComptimeValue::Optional(None) => {}
                _ => self.push_comptime_type_mismatch(span, "optional"),
            },
            Some(TyKind::ErrorUnion { error, value: ok }) => {
                let ComptimeValue::ErrorUnion(value) = value else {
                    self.push_comptime_type_mismatch(span, "error union");
                    return;
                };
                match value {
                    Ok(value) => self.validate_runtime_typed_value(span, value, ok),
                    Err(value) => self.validate_runtime_typed_value(span, value, error),
                }
            }
            Some(TyKind::Nominal { .. }) => {
                self.validate_nominal_struct_value(span, value, ty);
            }
            _ => {}
        }
    }

    fn validate_nominal_struct_value(
        &mut self,
        span: Span,
        value: &ComptimeValue,
        ty: InternedTyId,
    ) {
        let ComptimeValue::Struct(values) = value else {
            self.push_comptime_type_mismatch(span, "struct");
            return;
        };
        let Some((def_id, args)) = self.expected_nominal_parts(ty) else {
            return;
        };
        if self.def_kind_of(def_id) != Some(DefKind::Struct) {
            return;
        }
        let Some(signature) = self.struct_signature_for(def_id) else {
            return;
        };
        let Some(field_tys) = self.comptime_struct_field_types(&signature, &args) else {
            return;
        };
        let field_set: FieldSetCheck<String> =
            check_value_field_set(values.keys().cloned(), field_tys.keys().cloned());
        for (name, field_ty) in &field_tys {
            if let Some(value) = values.get(name.as_str()) {
                self.validate_runtime_typed_value(span, value, *field_ty);
            }
        }
        for name in &field_set.missing_fields {
            self.push_comptime_missing_struct_field(span, name);
        }
        for field in &field_set.unknown_fields {
            self.push_comptime_extra_struct_field(span, &field.name);
        }
    }

    fn validate_primitive_typed_value(
        &mut self,
        span: Span,
        value: &ComptimeValue,
        primitive: PrimitiveTy,
    ) {
        match (value, primitive) {
            (ComptimeValue::Int(value), primitive) => {
                let Some((min, max)) =
                    primitive_integer_range_for_target(primitive, self.input.target.pointer_width)
                else {
                    self.push_comptime_primitive_mismatch(span, primitive);
                    return;
                };
                if *value < min || *value > max {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!(
                            "comptime integer value {value} is out of range for {}",
                            primitive_ty_name(primitive)
                        ),
                    ));
                }
            }
            (ComptimeValue::Bool(_), PrimitiveTy::Bool) => {}
            (ComptimeValue::Float(value), PrimitiveTy::F32) => {
                let value = *value as f32;
                if !value.is_finite() {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        "comptime float value is out of range for f32",
                    ));
                }
            }
            (ComptimeValue::Float(value), PrimitiveTy::F64) => {
                if !value.is_finite() {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        "comptime float value is out of range for f64",
                    ));
                }
            }
            (_, primitive) => {
                self.push_comptime_primitive_mismatch(span, primitive);
            }
        }
    }

    fn runtime_array_len(&mut self, ty: InternedTyId) -> Option<u64> {
        let Some(TyKind::Array { len, .. }) = self.ty_kind(ty) else {
            return None;
        };
        self.array_len_const_value(len)
    }

    fn runtime_array_accepts_comptime_string(
        &mut self,
        value: &ComptimeValue,
        ty: InternedTyId,
    ) -> bool {
        let ComptimeValue::String(value) = value else {
            return false;
        };
        if !self.runtime_array_is_char_array(ty) {
            return false;
        }
        self.runtime_array_len(ty)
            .is_none_or(|len| value.chars().count() as u64 == len)
    }

    fn runtime_array_is_char_array(&self, ty: InternedTyId) -> bool {
        let Some(TyKind::Array { elem, .. }) = self.ty_kind(ty) else {
            return false;
        };
        matches!(
            self.ty_kind(elem),
            Some(TyKind::Primitive(PrimitiveTy::Char))
        )
    }

    fn push_comptime_type_mismatch(&mut self, span: Span, expected: &str) {
        self.diagnostics.push(Diagnostic::error(
            span,
            format!("comptime value does not match expected {expected} type"),
        ));
    }

    fn push_comptime_missing_struct_field(&mut self, span: Span, name: &str) {
        self.diagnostics.push(Diagnostic::error(
            span,
            format!("comptime struct value is missing field `{name}`"),
        ));
    }

    fn push_comptime_extra_struct_field(&mut self, span: Span, name: &str) {
        self.diagnostics.push(Diagnostic::error(
            span,
            format!("comptime struct value has extra field `{name}`"),
        ));
    }

    fn push_comptime_primitive_mismatch(&mut self, span: Span, primitive: PrimitiveTy) {
        self.diagnostics.push(Diagnostic::error(
            span,
            format!(
                "comptime value does not match primitive type {}",
                primitive_ty_name(primitive)
            ),
        ));
    }

    fn with_execution_module<T>(
        &mut self,
        module_id: ModuleId,
        f: impl FnOnce(&mut Self) -> T,
    ) -> T {
        self.execution_module_overrides.push(module_id);
        let result = f(self);
        self.execution_module_overrides.pop();
        result
    }

    fn explicit_type_for_key(&mut self, key: ComptimeKey) -> Option<InternedTyId> {
        match key {
            ComptimeKey::Global(global_id) => {
                let signatures = self.signatures_for_module(global_id.module_id)?;
                signatures.comptimes.get(&global_id.def_id)?.explicit_type
            }
            ComptimeKey::Local(local_id) => self.find_local_binding_type(local_id),
        }
    }

    fn find_local_binding_type(&mut self, local_id: LocalId) -> Option<InternedTyId> {
        if let Some(initializer) = self.input.module.local_initializers().get(&local_id)
            && initializer.explicit_type().is_some()
        {
            return initializer.explicit_type();
        }
        let global_initializers = self
            .input
            .module
            .global_initializers()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for expr in &global_initializers {
            if let Some(ty) = self.find_local_binding_type_in_resolved_expr(expr, local_id) {
                return Some(ty);
            }
        }
        let local_initializers = self
            .input
            .module
            .local_initializers()
            .values()
            .map(|initializer| initializer.value().clone())
            .collect::<Vec<_>>();
        for expr in &local_initializers {
            if let Some(ty) = self.find_local_binding_type_in_resolved_expr(expr, local_id) {
                return Some(ty);
            }
        }
        let function_bodies = self
            .input
            .module
            .functions()
            .values()
            .map(|function| function.body().clone())
            .collect::<Vec<_>>();
        for body in &function_bodies {
            if let Some(ty) = self.find_local_binding_type_in_resolved_block(body, local_id) {
                return Some(ty);
            }
        }
        None
    }

    fn find_local_binding_type_in_resolved_block(
        &mut self,
        block: &ResolvedComptimeBlock,
        local_id: LocalId,
    ) -> Option<InternedTyId> {
        for stmt in block.stmts() {
            match stmt.kind() {
                ResolvedComptimeStmtKind::Binding(binding) if binding.local_id() == local_id => {
                    return binding.explicit_type();
                }
                ResolvedComptimeStmtKind::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    if let Some(ty) =
                        self.find_local_binding_type_in_resolved_block(then_branch, local_id)
                    {
                        return Some(ty);
                    }
                    if let Some(else_branch) = else_branch
                        && let Some(ty) =
                            self.find_local_binding_type_in_resolved_block(else_branch, local_id)
                    {
                        return Some(ty);
                    }
                }
                ResolvedComptimeStmtKind::ForIn(for_in) => {
                    if let Some(ty) =
                        self.find_local_binding_type_in_resolved_block(for_in.body(), local_id)
                    {
                        return Some(ty);
                    }
                }
                ResolvedComptimeStmtKind::While { body, .. }
                | ResolvedComptimeStmtKind::Loop { body } => {
                    if let Some(ty) = self.find_local_binding_type_in_resolved_block(body, local_id)
                    {
                        return Some(ty);
                    }
                }
                ResolvedComptimeStmtKind::Expr(expr)
                | ResolvedComptimeStmtKind::Return(Some(expr)) => {
                    if let Some(ty) = self.find_local_binding_type_in_resolved_expr(expr, local_id)
                    {
                        return Some(ty);
                    }
                }
                ResolvedComptimeStmtKind::Binding(_)
                | ResolvedComptimeStmtKind::Return(None)
                | ResolvedComptimeStmtKind::Break
                | ResolvedComptimeStmtKind::Continue => {}
            }
        }
        block
            .tail()
            .and_then(|tail| self.find_local_binding_type_in_resolved_expr(tail, local_id))
    }

    fn find_local_binding_type_in_resolved_expr(
        &mut self,
        expr: &ResolvedComptimeExpr,
        local_id: LocalId,
    ) -> Option<InternedTyId> {
        match expr.kind() {
            ResolvedComptimeExprKind::If {
                then_branch,
                else_branch,
                ..
            } => self
                .find_local_binding_type_in_resolved_block(then_branch, local_id)
                .or_else(|| {
                    else_branch.as_deref().and_then(|else_branch| {
                        self.find_local_binding_type_in_resolved_expr(else_branch, local_id)
                    })
                }),
            ResolvedComptimeExprKind::Switch(switch) => {
                if let Some(ty) = self.find_resolved_switch_pattern_local_type(switch, local_id) {
                    return Some(ty);
                }
                switch
                    .arms()
                    .iter()
                    .find_map(|arm| match arm.body().kind() {
                        ResolvedComptimeSwitchArmBodyKind::Expr(expr) => {
                            self.find_local_binding_type_in_resolved_expr(expr, local_id)
                        }
                        ResolvedComptimeSwitchArmBodyKind::Stmt(stmt) => match stmt.kind() {
                            ResolvedComptimeStmtKind::Binding(binding)
                                if binding.local_id() == local_id =>
                            {
                                binding.explicit_type()
                            }
                            ResolvedComptimeStmtKind::Expr(expr)
                            | ResolvedComptimeStmtKind::Return(Some(expr)) => {
                                self.find_local_binding_type_in_resolved_expr(expr, local_id)
                            }
                            _ => None,
                        },
                        ResolvedComptimeSwitchArmBodyKind::Block(block) => {
                            self.find_local_binding_type_in_resolved_block(block, local_id)
                        }
                    })
            }
            ResolvedComptimeExprKind::Block(block) => {
                self.find_local_binding_type_in_resolved_block(block, local_id)
            }
            _ => None,
        }
    }

    fn push_engine_error(&mut self, err: ComptimeError) {
        self.diagnostics
            .push(Diagnostic::error(err.span, err.message));
    }

    fn initializer_for_key(&self, key: ComptimeKey) -> Option<&ResolvedComptimeExpr> {
        match key {
            ComptimeKey::Global(global_id) => self.global_initializer(global_id),
            ComptimeKey::Local(local_id) => self.local_initializer(local_id),
        }
    }

    fn global_initializer(&self, global_id: GlobalDefId) -> Option<&ResolvedComptimeExpr> {
        if global_id.module_id == self.input.defs.module_id {
            self.input.module.global_initializers().get(&global_id)
        } else {
            self.input
                .program
                .modules?
                .get(&global_id.module_id)?
                .global_initializers()
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

    fn local_initializer(&self, local_id: LocalId) -> Option<&ResolvedComptimeExpr> {
        self.input
            .module
            .local_initializers()
            .get(&local_id)
            .map(|initializer| initializer.value())
    }

    fn call_local_value(&self, local_id: LocalId) -> Option<ComptimeValue> {
        self.call_locals
            .iter()
            .rev()
            .find_map(|frame| frame.locals.get(&local_id).cloned())
    }

    fn def_kind_of(&self, global_id: GlobalDefId) -> Option<DefKind> {
        self.global_defs(global_id.module_id)?
            .defs
            .get(global_id.def_id)
            .map(|def| def.kind)
    }

    fn resolved_comptime_function(&self, callee: &ResolvedComptimeExpr) -> Option<GlobalDefId> {
        if let Some(ComptimeNameResolution::Global(global_id)) = callee.name_resolution()
            && self.def_kind_of(global_id) == Some(DefKind::Function)
        {
            return Some(global_id);
        }
        None
    }

    fn comptime_function_body(
        &self,
        def_id: GlobalDefId,
    ) -> Option<&nia_comptime_ir::ResolvedComptimeFunction> {
        if def_id.module_id == self.input.defs.module_id {
            self.input.module.functions().get(&def_id)
        } else {
            self.input
                .program
                .modules?
                .get(&def_id.module_id)?
                .functions()
                .get(&def_id)
        }
    }

    fn current_execution_module_id(&self) -> ModuleId {
        if let Some(module_id) = self.execution_module_overrides.last().copied() {
            return module_id;
        }
        self.call_locals
            .iter()
            .rev()
            .find_map(|frame| frame.module_id)
            .unwrap_or(self.input.defs.module_id)
    }

    fn interner_for_module(&self, module_id: ModuleId) -> Option<&TyInterner> {
        self.working_interners.get(&module_id)
    }

    fn source_interner_for_module(&self, module_id: ModuleId) -> Option<&TyInterner> {
        if module_id == self.input.defs.module_id {
            Some(self.input.interner)
        } else {
            Some(&self.type_normalization_for_module(module_id)?.interner)
        }
    }

    fn ensure_working_interner(&mut self, module_id: ModuleId) -> Option<()> {
        if self.working_interners.contains_key(&module_id) {
            return Some(());
        }
        let interner = self.source_interner_for_module(module_id)?.clone();
        self.working_interners.insert(module_id, interner);
        Some(())
    }

    fn signatures_for_module(&self, module_id: ModuleId) -> Option<&ItemSignatures> {
        if module_id == self.input.defs.module_id {
            Some(self.input.signatures)
        } else {
            self.input.program.signatures?.get(&module_id)
        }
    }

    fn type_normalization_for_module(
        &self,
        module_id: ModuleId,
    ) -> Option<&nia_type_normalize::TypeNormalization> {
        if module_id == self.input.defs.module_id {
            return None;
        }
        self.input.program.type_normalizations?.get(&module_id)
    }

    fn normalized_for_module(
        &self,
        module_id: ModuleId,
    ) -> Option<&HashMap<nia_ids::InternedTyId, nia_ids::InternedTyId>> {
        if module_id == self.input.defs.module_id {
            Some(self.input.normalized)
        } else {
            Some(&self.type_normalization_for_module(module_id)?.normalized)
        }
    }

    fn resolve_layout_builtin_for_ty(
        &mut self,
        span: Span,
        builtin: LayoutBuiltin,
        ty: nia_ids::InternedTyId,
    ) -> Result<ComptimeValue, ComptimeError> {
        let module_id = self.current_execution_module_id();
        let layout_array_lengths = self.program_array_lengths_for_layout(ty);
        if self.ensure_working_interner(module_id).is_none() {
            return Err(ComptimeError {
                span,
                message: "cannot compute layout without module type interner".to_string(),
            });
        }
        let Some(defs) = self.global_defs(module_id) else {
            return Err(ComptimeError {
                span,
                message: "cannot compute layout without module definitions".to_string(),
            });
        };
        let Some(signatures) = self.signatures_for_module(module_id) else {
            return Err(ComptimeError {
                span,
                message: "cannot compute layout without module signatures".to_string(),
            });
        };
        let Some(interner) = self.interner_for_module(module_id) else {
            return Err(ComptimeError {
                span,
                message: "cannot compute layout without module type interner".to_string(),
            });
        };
        let Some(normalized) = self.normalized_for_module(module_id) else {
            return Err(ComptimeError {
                span,
                message: "cannot compute layout without normalized module types".to_string(),
            });
        };
        let array_lengths = |id| layout_array_lengths.get(&id).copied();
        let layout_query =
            |module_id| self.compute_program_layout(module_id, &layout_array_lengths);
        let layouts = nia_layout::compute_layouts_with_program_context(
            defs,
            interner,
            signatures,
            normalized,
            &array_lengths,
            nia_layout::TargetDataLayout::LP64,
            nia_layout::ProgramLayoutContext {
                layouts: Some(&layout_query),
                array_lengths: Some(&array_lengths),
            },
        );
        let ty = normalized.get(&ty).copied().unwrap_or(ty);
        if let Some(TyKind::Nominal { def_id, args }) = self.ty_kind(ty)
            && (def_id.module_id != module_id || ty.interner_id != module_id)
            && let Some(layouts) =
                self.compute_program_layout(def_id.module_id, &layout_array_lengths)
            && let Some(layout) = layouts.nominal_type_layout(def_id, &args)
        {
            return Ok(ComptimeValue::Int(layout.builtin_value(builtin) as i128));
        }
        if ty.interner_id != module_id
            && let Some(layouts) =
                self.compute_program_layout(ty.interner_id, &layout_array_lengths)
            && let Some(layout) = layouts.types.get(&ty)
        {
            return Ok(ComptimeValue::Int(layout.builtin_value(builtin) as i128));
        }
        let Some(layout) = layouts.types.get(&ty) else {
            return Err(ComptimeError {
                span,
                message: format!(
                    "cannot compute layout for comptime builtin `@{}`",
                    builtin.name()
                ),
            });
        };
        Ok(ComptimeValue::Int(layout.builtin_value(builtin) as i128))
    }

    fn program_array_lengths_for_layout(
        &mut self,
        ty: InternedTyId,
    ) -> HashMap<GlobalConstExprId, u64> {
        let mut array_lengths = self.array_lengths.clone();
        let mut needed = HashSet::new();
        self.collect_array_len_const_exprs_in_ty(ty, &mut needed);
        for id in needed {
            if array_lengths.contains_key(&id) {
                continue;
            }
            if let Some(value) = self.eval_array_len_const_expr_id(id) {
                array_lengths.insert(id, value);
            }
        }
        array_lengths
    }

    fn collect_array_len_const_exprs_in_ty(
        &self,
        ty: InternedTyId,
        out: &mut HashSet<GlobalConstExprId>,
    ) {
        self.collect_array_len_const_exprs_in_ty_inner(ty, out, &mut HashSet::new());
    }

    fn collect_array_len_const_exprs_in_ty_inner(
        &self,
        ty: InternedTyId,
        out: &mut HashSet<GlobalConstExprId>,
        seen: &mut HashSet<InternedTyId>,
    ) {
        if !seen.insert(ty) {
            return;
        }
        match self.ty_kind(ty) {
            Some(TyKind::Array { len, elem }) => {
                if let ArrayLenTy::ConstExpr(id) = len {
                    out.insert(id);
                }
                self.collect_array_len_const_exprs_in_ty_inner(elem, out, seen);
            }
            Some(TyKind::Optional { elem })
            | Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::Slice { elem, .. }) => {
                self.collect_array_len_const_exprs_in_ty_inner(elem, out, seen);
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                self.collect_array_len_const_exprs_in_ty_inner(error, out, seen);
                self.collect_array_len_const_exprs_in_ty_inner(value, out, seen);
            }
            Some(TyKind::Range {
                bound: Some(bound), ..
            }) => {
                self.collect_array_len_const_exprs_in_ty_inner(bound, out, seen);
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                for param in params {
                    self.collect_array_len_const_exprs_in_ty_inner(param, out, seen);
                }
                self.collect_array_len_const_exprs_in_ty_inner(return_type, out, seen);
            }
            Some(TyKind::Nominal { def_id, args }) => {
                for arg in args {
                    self.collect_array_len_const_exprs_in_ty_inner(arg, out, seen);
                }
                let Some(signatures) = self.signatures_for_module(def_id.module_id) else {
                    return;
                };
                if let Some(signature) = signatures.structs.get(&def_id.def_id) {
                    for field in &signature.fields {
                        self.collect_array_len_const_exprs_in_ty_inner(field.ty, out, seen);
                    }
                }
                if let Some(signature) = signatures.unions.get(&def_id.def_id) {
                    for field in &signature.fields {
                        self.collect_array_len_const_exprs_in_ty_inner(field.ty, out, seen);
                    }
                }
            }
            Some(TyKind::BuiltinTrait { args, .. }) => {
                for arg in args {
                    self.collect_array_len_const_exprs_in_ty_inner(arg, out, seen);
                }
            }
            Some(TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            }) => {
                for arg in trait_args {
                    self.collect_array_len_const_exprs_in_ty_inner(arg, out, seen);
                }
                for binding in associated_type_bindings {
                    self.collect_array_len_const_exprs_in_ty_inner(binding.ty, out, seen);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.collect_array_len_const_exprs_in_ty_inner(self_ty, out, seen);
                for arg in trait_args {
                    self.collect_array_len_const_exprs_in_ty_inner(arg, out, seen);
                }
            }
            Some(
                TyKind::Range { bound: None, .. }
                | TyKind::Error
                | TyKind::ComptimeOnly
                | TyKind::GenericParam(_)
                | TyKind::Primitive(_),
            )
            | None => {}
        }
    }

    fn compute_program_layout(
        &self,
        module_id: ModuleId,
        array_lengths: &HashMap<GlobalConstExprId, u64>,
    ) -> Option<nia_layout::Layouts> {
        let defs = self.global_defs(module_id)?;
        let signatures = self.signatures_for_module(module_id)?;
        let interner = self.source_interner_for_module(module_id)?;
        let normalized = self.normalized_for_module(module_id)?;
        let array_lengths_for_layout = |id: GlobalConstExprId| array_lengths.get(&id).copied();
        let layout_query = |module_id| self.compute_program_layout(module_id, array_lengths);
        Some(nia_layout::compute_layouts_with_program_context(
            defs,
            interner,
            signatures,
            normalized,
            &array_lengths_for_layout,
            nia_layout::TargetDataLayout::LP64,
            nia_layout::ProgramLayoutContext {
                layouts: Some(&layout_query),
                array_lengths: Some(&array_lengths_for_layout),
            },
        ))
    }

    fn substitute_ty_generics(&mut self, ty: InternedTyId) -> InternedTyId {
        let module_id = self.current_execution_module_id();
        let substitutions = self
            .call_locals
            .iter()
            .flat_map(|frame| frame.type_substitutions.iter())
            .map(|(name, ty)| (name.clone(), *ty))
            .collect::<HashMap<_, _>>();
        let interner = self
            .working_interners
            .get_mut(&module_id)
            .expect("working interner must exist for current execution module");
        substitute_ty_generics_in_interner(interner, ty, &|name| substitutions.get(name).copied())
    }

    fn instantiate_resolved_function_generics(
        &mut self,
        span: Span,
        signature_module_id: ModuleId,
        signature: &FunctionSignature,
        type_args: &[ResolvedComptimeTypeArg],
        arg_exprs: &[ResolvedComptimeExpr],
    ) -> Result<HashMap<String, InternedTyId>, ComptimeError> {
        if self.ensure_working_interner(signature_module_id).is_none() {
            return Err(ComptimeError {
                span,
                message: "cannot instantiate comptime function without module type interner"
                    .to_string(),
            });
        }
        if !type_args.is_empty()
            && let ArityCheck::Mismatch { actual, .. } =
                check_exact_arity(signature.generics.len(), type_args.len())
        {
            return Err(ComptimeError {
                span,
                message: format!(
                    "generic argument count mismatch for comptime function: expected {}, got {}",
                    signature.generics.len(),
                    actual
                ),
            });
        }
        let mut substitutions = HashMap::new();
        if type_args.is_empty() {
            for (param, arg_expr) in signature.params.iter().zip(arg_exprs) {
                let expected = self.comptime_expected_param_type(
                    signature_module_id,
                    param.ty,
                    &substitutions,
                );
                let Some(arg_ty) = self.resolved_comptime_arg_runtime_type(arg_expr, expected)
                else {
                    continue;
                };
                self.infer_generics_from_tys(
                    span,
                    signature_module_id,
                    param.ty,
                    arg_ty,
                    &mut substitutions,
                )?;
            }
            for generic in &signature.generics {
                if !substitutions.contains_key(generic) {
                    return Err(ComptimeError {
                        span,
                        message: format!("cannot infer comptime generic type argument `{generic}`"),
                    });
                }
            }
        } else {
            for (generic, arg) in signature.generics.iter().zip(type_args) {
                let imported =
                    self.import_ty_into_module(arg.span(), arg.ty(), signature_module_id)?;
                substitutions.insert(generic.clone(), imported);
            }
        }
        Ok(substitutions)
    }

    fn comptime_expected_param_type(
        &mut self,
        module_id: ModuleId,
        ty: InternedTyId,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Option<InternedTyId> {
        self.ensure_working_interner(module_id)?;
        let interner = self.working_interners.get_mut(&module_id)?;
        Some(substitute_ty_generics_in_interner(
            interner,
            ty,
            &|generic| substitutions.get(generic).copied(),
        ))
    }

    fn resolved_comptime_arg_runtime_type(
        &mut self,
        expr: &ResolvedComptimeExpr,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        self.resolved_comptime_expr_type(expr, expected)
            .and_then(|ty| ty.runtime())
    }

    fn probe_resolved_comptime_int_expr(&mut self, expr: &ResolvedComptimeExpr) -> Option<i128> {
        nia_comptime_engine::eval_resolved_comptime_int_expr(expr, self).ok()
    }

    fn probe_resolved_comptime_array_len_expr(
        &mut self,
        expr: &ResolvedComptimeExpr,
    ) -> Option<u64> {
        nia_comptime_engine::eval_resolved_comptime_array_len_expr(expr, self).ok()
    }

    fn probe_type_generic_inference(
        &mut self,
        span: Span,
        expected: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
    ) -> Option<()> {
        self.infer_generics_from_tys(
            span,
            self.current_execution_module_id(),
            expected,
            actual,
            substitutions,
        )
        .ok()
    }

    fn comptime_name_resolution_type(
        &mut self,
        resolution: ComptimeNameResolution,
    ) -> Option<ComptimeValueType> {
        match resolution {
            ComptimeNameResolution::Local(local_id) => self
                .call_local_type(local_id)
                .or_else(|| {
                    let ty = self
                        .typed_value_for_key(ComptimeKey::Local(local_id))
                        .map(|typed| typed.ty.clone())?;
                    self.import_comptime_value_type(ty, self.current_execution_module_id())
                })
                .or_else(|| {
                    self.explicit_type_for_key(ComptimeKey::Local(local_id))
                        .and_then(|ty| {
                            self.import_ty_into_module_or_none(
                                ty,
                                self.current_execution_module_id(),
                            )
                        })
                        .map(ComptimeValueType::Runtime)
                }),
            ComptimeNameResolution::Global(global_id) => self
                .typed_value_for_key(ComptimeKey::Global(global_id))
                .map(|typed| typed.ty.clone())
                .and_then(|ty| {
                    self.import_comptime_value_type(ty, self.current_execution_module_id())
                })
                .or_else(|| {
                    self.explicit_type_for_key(ComptimeKey::Global(global_id))
                        .and_then(|ty| {
                            self.import_ty_into_module_or_none(
                                ty,
                                self.current_execution_module_id(),
                            )
                        })
                        .map(ComptimeValueType::Runtime)
                }),
        }
    }

    fn resolved_comptime_expr_type(
        &mut self,
        expr: &ResolvedComptimeExpr,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
        match expr.kind() {
            ResolvedComptimeExprKind::Name(resolution) => {
                self.comptime_name_resolution_type(*resolution)
            }
            ResolvedComptimeExprKind::Integer(text) => {
                integer_literal_suffix_ty(text).map(|primitive| {
                    ComptimeValueType::Runtime(
                        self.source_interner_for_module(self.current_execution_module_id())
                            .unwrap_or(self.input.interner)
                            .primitive(primitive),
                    )
                })
            }
            ResolvedComptimeExprKind::Float(text) => {
                let primitive = float_literal_suffix_ty(text).unwrap_or(PrimitiveTy::F64);
                Some(ComptimeValueType::Runtime(
                    self.current_runtime_primitive_type(primitive),
                ))
            }
            ResolvedComptimeExprKind::Char(_) => Some(ComptimeValueType::Runtime(
                self.current_runtime_primitive_type(PrimitiveTy::Char),
            )),
            ResolvedComptimeExprKind::ByteChar(_) => Some(ComptimeValueType::Runtime(
                self.current_runtime_primitive_type(PrimitiveTy::U8),
            )),
            ResolvedComptimeExprKind::String(literal) => self.comptime_string_literal_type(literal),
            ResolvedComptimeExprKind::ByteString(literal) => self
                .comptime_byte_string_literal_type(
                    nia_comptime_engine::eval_byte_string_literal(literal)?.len() as u64,
                ),
            ResolvedComptimeExprKind::CString(literal) => self.comptime_byte_string_literal_type(
                nia_comptime_engine::eval_c_string_literal(literal)?.len() as u64,
            ),
            ResolvedComptimeExprKind::Bool(_) => Some(ComptimeValueType::Runtime(
                self.current_runtime_primitive_type(PrimitiveTy::Bool),
            )),
            ResolvedComptimeExprKind::ArrayLiteral { ty: Some(ty), .. }
            | ResolvedComptimeExprKind::StructLiteral { ty: Some(ty), .. } => {
                Some(ComptimeValueType::Runtime(*ty))
            }
            ResolvedComptimeExprKind::ArrayLiteral { ty: None, elems } => {
                self.resolved_comptime_array_literal_type(elems, expected)
            }
            ResolvedComptimeExprKind::StructLiteral { ty: None, fields } => {
                self.resolved_comptime_struct_literal_type(expr.span(), fields, expected)
            }
            ResolvedComptimeExprKind::OptionalSome { expr: inner } => {
                let expected_elem = expected.and_then(|expected| match self.ty_kind(expected) {
                    Some(TyKind::Optional { elem }) => Some(elem),
                    _ => None,
                });
                let elem = self.resolved_comptime_arg_runtime_type(inner, expected_elem)?;
                self.comptime_runtime_type(
                    elem,
                    |elem| TyKind::Optional { elem },
                    self.current_execution_module_id(),
                )
                .map(ComptimeValueType::Runtime)
            }
            ResolvedComptimeExprKind::ErrorOk { expr: inner } => {
                let (error, value) = self.expected_error_union_parts(expected?)?;
                let actual_value = self.resolved_comptime_arg_runtime_type(inner, Some(value))?;
                self.comptime_error_union_type(error, actual_value)
                    .map(ComptimeValueType::Runtime)
            }
            ResolvedComptimeExprKind::ErrorErr { expr: inner } => {
                let (error, value) = self.expected_error_union_parts(expected?)?;
                let actual_error = self.resolved_comptime_arg_runtime_type(inner, Some(error))?;
                self.comptime_error_union_type(actual_error, value)
                    .map(ComptimeValueType::Runtime)
            }
            ResolvedComptimeExprKind::Try { expr: inner } => {
                let inner_ty = self.resolved_comptime_arg_runtime_type(inner, None)?;
                let payload = match self.ty_kind(inner_ty)? {
                    TyKind::Optional { elem } => elem,
                    TyKind::ErrorUnion { value, .. } => value,
                    _ => return None,
                };
                self.import_ty_into_module_or_none(payload, self.current_execution_module_id())
                    .map(ComptimeValueType::Runtime)
            }
            ResolvedComptimeExprKind::Field { lhs, name } => {
                let lhs_ty = self.resolved_comptime_expr_type(lhs, None)?;
                self.comptime_field_type(lhs_ty, name)
            }
            ResolvedComptimeExprKind::Len { lhs } => {
                let lhs_ty = self.resolved_comptime_expr_type(lhs, None)?;
                self.comptime_len_type(lhs_ty)
            }
            ResolvedComptimeExprKind::RangeIter { lhs } => {
                let lhs_ty = self.resolved_comptime_expr_type(lhs, None)?;
                self.comptime_range_iter_type(lhs_ty)
            }
            ResolvedComptimeExprKind::Cast { expr: inner, ty } => {
                self.resolved_comptime_cast_type(inner, *ty)
            }
            ResolvedComptimeExprKind::Index { lhs, index } => {
                let lhs_ty = self.resolved_comptime_expr_type(lhs, None)?;
                self.resolved_comptime_index_type(expr.span(), lhs_ty, index)
            }
            ResolvedComptimeExprKind::Slice { lhs, range } => {
                let lhs_ty = self.resolved_comptime_expr_type(lhs, None)?;
                self.resolved_comptime_slice_type(lhs_ty, range, expected)
            }
            ResolvedComptimeExprKind::Range(range) => {
                self.resolved_comptime_range_type(range, expected)
            }
            ResolvedComptimeExprKind::Binary { lhs, op, rhs } => {
                self.resolved_comptime_binary_expr_type(lhs, *op, rhs)
            }
            ResolvedComptimeExprKind::Unary { op, expr: inner } => {
                self.resolved_comptime_unary_expr_type(*op, inner)
            }
            ResolvedComptimeExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.resolved_comptime_if_expr_type(
                cond,
                then_branch,
                else_branch.as_deref(),
                expected,
            ),
            ResolvedComptimeExprKind::Switch(switch) => {
                self.resolved_comptime_switch_expr_type(switch, expected)
            }
            ResolvedComptimeExprKind::Block(block) => {
                self.resolved_comptime_block_tail_type(block, expected)
            }
            ResolvedComptimeExprKind::BuiltinValue(ValueBuiltin::Builtin) => {
                Some(self.builtin_comptime_type())
            }
            ResolvedComptimeExprKind::Call { callee, args, .. }
                if args.is_empty()
                    && matches!(
                        callee.kind(),
                        ResolvedComptimeExprKind::BuiltinValue(ValueBuiltin::Builtin)
                    ) =>
            {
                Some(self.builtin_comptime_type())
            }
            ResolvedComptimeExprKind::Call {
                callee,
                type_args,
                args,
            } => self
                .resolved_comptime_call_return_type(expr.span(), callee, type_args, args)
                .map(ComptimeValueType::Runtime),
            ResolvedComptimeExprKind::LayoutBuiltin { .. }
            | ResolvedComptimeExprKind::Null
            | ResolvedComptimeExprKind::Assign(_) => None,
        }
    }

    fn find_resolved_switch_pattern_local_type(
        &mut self,
        switch: &ResolvedComptimeSwitch,
        local_id: LocalId,
    ) -> Option<InternedTyId> {
        let target_ty = self.resolved_comptime_arg_runtime_type(switch.target(), None)?;
        for arm in switch.arms() {
            for pattern in arm.patterns() {
                if resolved_switch_pattern_local_id(pattern) == Some(local_id) {
                    return self.resolved_switch_pattern_binding_type(pattern, target_ty);
                }
            }
        }
        None
    }

    fn resolved_switch_pattern_binding_type(
        &self,
        pattern: &ResolvedComptimeSwitchPattern,
        target_ty: InternedTyId,
    ) -> Option<InternedTyId> {
        match pattern.kind() {
            ResolvedComptimeSwitchPatternKind::OptionalSome { .. } => {
                match self.ty_kind(target_ty)? {
                    TyKind::Optional { elem } => Some(elem),
                    _ => None,
                }
            }
            ResolvedComptimeSwitchPatternKind::ErrorOk { .. } => match self.ty_kind(target_ty)? {
                TyKind::ErrorUnion { value, .. } => Some(value),
                _ => None,
            },
            ResolvedComptimeSwitchPatternKind::ErrorErr { .. } => match self.ty_kind(target_ty)? {
                TyKind::ErrorUnion { error, .. } => Some(error),
                _ => None,
            },
            ResolvedComptimeSwitchPatternKind::Default
            | ResolvedComptimeSwitchPatternKind::OptionalNull { .. }
            | ResolvedComptimeSwitchPatternKind::Expr(_)
            | ResolvedComptimeSwitchPatternKind::Range { .. } => None,
        }
    }

    fn comptime_field_type(
        &mut self,
        lhs: ComptimeValueType,
        name: &str,
    ) -> Option<ComptimeValueType> {
        match &lhs {
            ComptimeValueType::Struct(_) => lhs.structural_field(name).cloned(),
            ComptimeValueType::Runtime(ty) => self
                .comptime_nominal_struct_field_type(*ty, name)
                .map(ComptimeValueType::Runtime),
            ComptimeValueType::Array { .. }
            | ComptimeValueType::Int
            | ComptimeValueType::Bool
            | ComptimeValueType::String => None,
        }
    }

    fn comptime_len_type(&mut self, lhs: ComptimeValueType) -> Option<ComptimeValueType> {
        match lhs {
            ComptimeValueType::Array { .. } => Some(ComptimeValueType::Runtime(
                self.current_runtime_primitive_type(PrimitiveTy::Usize),
            )),
            ComptimeValueType::Runtime(ty) => match self.ty_kind(ty)? {
                TyKind::Array { .. } => Some(ComptimeValueType::Runtime(
                    self.current_runtime_primitive_type(PrimitiveTy::Usize),
                )),
                _ => None,
            },
            ComptimeValueType::Struct(_)
            | ComptimeValueType::Int
            | ComptimeValueType::Bool
            | ComptimeValueType::String => None,
        }
    }

    fn comptime_range_iter_type(&mut self, lhs: ComptimeValueType) -> Option<ComptimeValueType> {
        let ComptimeValueType::Runtime(ty) = lhs else {
            return None;
        };
        match self.ty_kind(ty)? {
            TyKind::Range {
                kind: RangeTyKind::Exclusive | RangeTyKind::Inclusive | RangeTyKind::From,
                bound: Some(_),
            } => Some(ComptimeValueType::Runtime(ty)),
            _ => None,
        }
    }

    fn resolved_comptime_index_type(
        &mut self,
        span: Span,
        lhs: ComptimeValueType,
        index: &ResolvedComptimeExpr,
    ) -> Option<ComptimeValueType> {
        match lhs {
            ComptimeValueType::Array { .. } => {
                let (elem, len) = lhs.array_elem()?;
                if let Some(len) = len
                    && self
                        .probe_resolved_comptime_array_len_expr(index)
                        .is_some_and(|index| index >= len)
                {
                    return None;
                }
                Some(elem.clone())
            }
            ComptimeValueType::Runtime(ty) => {
                self.resolved_comptime_runtime_index_type(span, ty, index)
            }
            ComptimeValueType::Struct(_)
            | ComptimeValueType::Int
            | ComptimeValueType::Bool
            | ComptimeValueType::String => None,
        }
    }

    fn resolved_comptime_runtime_index_type(
        &mut self,
        _span: Span,
        lhs: InternedTyId,
        index: &ResolvedComptimeExpr,
    ) -> Option<ComptimeValueType> {
        let (len, elem) = match self.ty_kind(lhs)? {
            TyKind::Array { len, elem } => (Some(len), elem),
            TyKind::Slice { elem, .. } => (None, elem),
            _ => return None,
        };
        if let Some(ArrayLenTy::ConstValue(len)) = len {
            let index = self.probe_resolved_comptime_array_len_expr(index)?;
            if index >= len {
                return None;
            }
        } else {
            self.probe_resolved_comptime_int_expr(index)?;
        }
        self.import_ty_into_module_or_none(elem, self.current_execution_module_id())
            .map(ComptimeValueType::Runtime)
    }

    fn resolved_comptime_slice_type(
        &mut self,
        lhs: ComptimeValueType,
        range: &nia_comptime_ir::ResolvedComptimeSliceRange,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
        match lhs {
            ComptimeValueType::Array { .. } => {
                let (elem, len) = lhs.array_elem()?;
                let expected_len = self.expected_const_array_len(expected);
                let actual_len = self.resolved_comptime_slice_len(len, expected_len, range)?;
                self.comptime_slice_result_type(elem.clone(), actual_len, expected)
            }
            ComptimeValueType::Runtime(ty) => {
                self.resolved_comptime_runtime_slice_type(ty, range, expected)
            }
            ComptimeValueType::Struct(_)
            | ComptimeValueType::Int
            | ComptimeValueType::Bool
            | ComptimeValueType::String => None,
        }
    }

    fn resolved_comptime_runtime_slice_type(
        &mut self,
        lhs: InternedTyId,
        range: &nia_comptime_ir::ResolvedComptimeSliceRange,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
        let (len, elem) = match self.ty_kind(lhs)? {
            TyKind::Array { len, elem } => (Some(len), elem),
            TyKind::Slice { elem, .. } => (None, elem),
            _ => return None,
        };
        let known_len = len.and_then(|len| self.array_len_const_value(len));
        let expected_len = self.expected_const_array_len(expected);
        let actual_len = self.resolved_comptime_slice_len(known_len, expected_len, range)?;
        let elem = self.import_ty_into_module_or_none(elem, self.current_execution_module_id())?;
        self.comptime_slice_result_type(ComptimeValueType::Runtime(elem), actual_len, expected)
    }

    fn comptime_slice_result_type(
        &mut self,
        elem: ComptimeValueType,
        actual_len: u64,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
        if let Some((expected_len, expected_elem)) =
            expected.and_then(|expected| self.expected_array_parts(expected))
            && elem.runtime() == Some(expected_elem)
        {
            let len = self.comptime_array_literal_len(Some(expected_len), Some(actual_len))?;
            return self
                .comptime_runtime_type(
                    expected_elem,
                    |elem| TyKind::Array { len, elem },
                    self.current_execution_module_id(),
                )
                .map(ComptimeValueType::Runtime);
        }
        Some(ComptimeValueType::Array {
            elem: Box::new(elem),
            len: Some(actual_len),
        })
    }

    fn resolved_comptime_slice_len(
        &mut self,
        source_len: Option<u64>,
        expected_len: Option<u64>,
        range: &nia_comptime_ir::ResolvedComptimeSliceRange,
    ) -> Option<u64> {
        let start = match range.start() {
            Some(start) => self.probe_resolved_comptime_array_len_expr(start)?,
            None => 0,
        };
        let mut end = match range.end() {
            Some(end) => self.probe_resolved_comptime_array_len_expr(end)?,
            None => source_len.or_else(|| expected_len.and_then(|len| start.checked_add(len)))?,
        };
        if range.is_inclusive() {
            end = end.checked_add(1)?;
        }
        if start > end {
            return None;
        }
        if let Some(source_len) = source_len
            && end > source_len
        {
            return None;
        }
        Some(end - start)
    }

    fn expected_const_array_len(&mut self, expected: Option<InternedTyId>) -> Option<u64> {
        let expected = expected?;
        let TyKind::Array { len, .. } = self.ty_kind(expected)? else {
            return None;
        };
        self.array_len_const_value(len)
    }

    fn array_len_const_value(&mut self, len: ArrayLenTy) -> Option<u64> {
        match len {
            ArrayLenTy::ConstValue(len) => Some(len),
            ArrayLenTy::ConstExpr(id) => self
                .array_lengths
                .get(&id)
                .copied()
                .or_else(|| self.eval_array_len_const_expr_id(id)),
            ArrayLenTy::Infer | ArrayLenTy::Builtin { .. } => None,
        }
    }

    fn eval_array_len_const_expr_id(&mut self, id: GlobalConstExprId) -> Option<u64> {
        let expr = if id.module_id == self.input.defs.module_id {
            self.input.module.const_exprs().get(&id)?.clone()
        } else {
            self.input
                .program
                .modules?
                .get(&id.module_id)?
                .const_exprs()
                .get(&id)?
                .clone()
        };
        let value = self.with_execution_module(id.module_id, |this| {
            this.eval_resolved_array_len_expr(&expr)
        })?;
        self.array_lengths.insert(id, value);
        Some(value)
    }

    fn import_comptime_value_type(
        &mut self,
        ty: ComptimeValueType,
        target_module_id: ModuleId,
    ) -> Option<ComptimeValueType> {
        match ty {
            ComptimeValueType::Runtime(ty) => self
                .import_ty_into_module_or_none(ty, target_module_id)
                .map(ComptimeValueType::Runtime),
            ComptimeValueType::Array { elem, len } => Some(ComptimeValueType::Array {
                elem: Box::new(self.import_comptime_value_type(*elem, target_module_id)?),
                len,
            }),
            ComptimeValueType::Struct(fields) => fields
                .into_iter()
                .map(|field| {
                    Some(ComptimeValueFieldType {
                        name: field.name,
                        ty: self.import_comptime_value_type(field.ty, target_module_id)?,
                    })
                })
                .collect::<Option<Vec<_>>>()
                .map(ComptimeValueType::Struct),
            ComptimeValueType::Int => Some(ComptimeValueType::Int),
            ComptimeValueType::Bool => Some(ComptimeValueType::Bool),
            ComptimeValueType::String => Some(ComptimeValueType::String),
        }
    }

    fn resolved_comptime_array_literal_type(
        &mut self,
        elems: &ResolvedComptimeArrayElements,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
        let expected_parts = expected.and_then(|expected| self.expected_array_parts(expected));
        if expected_parts.is_none()
            && let Some(ty) = self.structural_resolved_comptime_array_literal_type(elems)
        {
            return Some(ty);
        }
        let (elem_ty, actual_len) = match elems.kind() {
            ResolvedComptimeArrayElementsKind::List(elems) => {
                let expected_elem = expected_parts.as_ref().map(|(_, elem)| *elem);
                let elem_ty = self.resolved_comptime_array_list_elem_type(elems, expected_elem)?;
                (elem_ty, Some(elems.len() as u64))
            }
            ResolvedComptimeArrayElementsKind::Repeat { value, count } => {
                let expected_elem = expected_parts.as_ref().map(|(_, elem)| *elem);
                let elem_ty = self.resolved_comptime_arg_runtime_type(value, expected_elem)?;
                let actual_len = Some(self.probe_resolved_comptime_array_len_expr(count)?);
                (elem_ty, actual_len)
            }
        };
        let len =
            self.comptime_array_literal_len(expected_parts.map(|(len, _)| len), actual_len)?;
        self.comptime_runtime_type(
            elem_ty,
            |elem| TyKind::Array { len, elem },
            self.current_execution_module_id(),
        )
        .map(ComptimeValueType::Runtime)
    }

    fn structural_resolved_comptime_array_literal_type(
        &mut self,
        elems: &ResolvedComptimeArrayElements,
    ) -> Option<ComptimeValueType> {
        let (elem_ty, len) = match elems.kind() {
            ResolvedComptimeArrayElementsKind::List(elems) => {
                let first = elems.first()?;
                let elem_ty = self.resolved_comptime_expr_type(first, None)?;
                for elem in &elems[1..] {
                    if self.resolved_comptime_expr_type(elem, None)? != elem_ty {
                        return None;
                    }
                }
                (elem_ty, Some(elems.len() as u64))
            }
            ResolvedComptimeArrayElementsKind::Repeat { value, count } => {
                let elem_ty = self.resolved_comptime_expr_type(value, None)?;
                let len = self.probe_resolved_comptime_array_len_expr(count)?;
                (elem_ty, Some(len))
            }
        };
        Some(ComptimeValueType::Array {
            elem: Box::new(elem_ty),
            len,
        })
    }

    fn expected_array_parts(&self, expected: InternedTyId) -> Option<(ArrayLenTy, InternedTyId)> {
        match self.ty_kind(expected)? {
            TyKind::Array { len, elem } => Some((len, elem)),
            _ => None,
        }
    }

    fn resolved_comptime_array_list_elem_type(
        &mut self,
        elems: &[ResolvedComptimeExpr],
        expected_elem: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let (anchor_index, elem_ty) =
            self.resolved_comptime_array_list_anchor_elem_type(elems, expected_elem)?;
        for (index, elem) in elems.iter().enumerate() {
            if index == anchor_index {
                continue;
            }
            let actual = self.resolved_comptime_arg_runtime_type(elem, Some(elem_ty))?;
            if actual != elem_ty {
                return None;
            }
        }
        Some(elem_ty)
    }

    fn resolved_comptime_array_list_anchor_elem_type(
        &mut self,
        elems: &[ResolvedComptimeExpr],
        expected_elem: Option<InternedTyId>,
    ) -> Option<(usize, InternedTyId)> {
        for (index, elem) in elems.iter().enumerate() {
            let expected_ty = expected_elem
                .and_then(|expected| self.resolved_comptime_arg_runtime_type(elem, Some(expected)))
                .filter(|ty| !self.type_contains_generic(*ty));
            if let Some(ty) =
                expected_ty.or_else(|| self.resolved_comptime_arg_runtime_type(elem, None))
                && !self.type_contains_generic(ty)
            {
                return Some((index, ty));
            }
        }
        None
    }

    fn type_contains_generic(&self, ty: InternedTyId) -> bool {
        let mut seen = HashSet::new();
        self.type_contains_generic_inner(ty, &mut seen)
    }

    fn type_contains_generic_inner(
        &self,
        ty: InternedTyId,
        seen: &mut HashSet<InternedTyId>,
    ) -> bool {
        if !seen.insert(ty) {
            return false;
        }
        match self.ty_kind(ty) {
            Some(TyKind::GenericParam(_)) => true,
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::Array { elem, .. })
            | Some(TyKind::Optional { elem }) => self.type_contains_generic_inner(elem, seen),
            Some(TyKind::Range { bound, .. }) => {
                bound.is_some_and(|bound| self.type_contains_generic_inner(bound, seen))
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                params
                    .into_iter()
                    .any(|param| self.type_contains_generic_inner(param, seen))
                    || self.type_contains_generic_inner(return_type, seen)
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                self.type_contains_generic_inner(error, seen)
                    || self.type_contains_generic_inner(value, seen)
            }
            Some(TyKind::Nominal { args, .. }) | Some(TyKind::BuiltinTrait { args, .. }) => args
                .into_iter()
                .any(|arg| self.type_contains_generic_inner(arg, seen)),
            Some(TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            }) => {
                trait_args
                    .into_iter()
                    .any(|arg| self.type_contains_generic_inner(arg, seen))
                    || associated_type_bindings
                        .into_iter()
                        .any(|binding| self.type_contains_generic_inner(binding.ty, seen))
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.type_contains_generic_inner(self_ty, seen)
                    || trait_args
                        .into_iter()
                        .any(|arg| self.type_contains_generic_inner(arg, seen))
            }
            Some(TyKind::Error | TyKind::ComptimeOnly | TyKind::Primitive(_)) | None => false,
        }
    }

    fn comptime_array_literal_len(
        &self,
        expected: Option<ArrayLenTy>,
        actual: Option<u64>,
    ) -> Option<ArrayLenTy> {
        match check_array_literal_len(expected, None, actual) {
            ArrayLiteralLenCheck::Accepted(len) => Some(len),
            ArrayLiteralLenCheck::Mismatch { .. } | ArrayLiteralLenCheck::Unknown => None,
        }
    }

    fn resolved_comptime_struct_literal_type(
        &mut self,
        span: Span,
        fields: &[ResolvedComptimeFieldInit],
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
        let Some(expected) = expected else {
            return self.structural_resolved_comptime_struct_literal_type(fields);
        };
        let (def_id, expected_args) = self.expected_nominal_parts(expected)?;
        if self.def_kind_of(def_id) != Some(DefKind::Struct) {
            return None;
        }
        let signature = self.struct_signature_for(def_id)?;
        let field_tys = self.comptime_struct_field_types(&signature, &expected_args)?;
        let field_set = check_required_field_set(
            fields
                .iter()
                .map(|field| NamedField::new(field.span(), field.name())),
            field_tys.keys().map(String::as_str),
        );
        if !field_set.is_valid() {
            return None;
        }
        let mut substitutions = HashMap::new();
        for field in fields {
            let expected_field = *field_tys.get(field.name())?;
            if let Some(actual_field) =
                self.resolved_comptime_struct_field_actual_type(field.value(), expected_field)
            {
                self.probe_type_generic_inference(
                    span,
                    expected_field,
                    actual_field,
                    &mut substitutions,
                )?;
            }
        }
        for field in fields {
            let expected_field =
                self.substitute_current_ty_generics(*field_tys.get(field.name())?, &substitutions)?;
            let actual_field =
                self.resolved_comptime_arg_runtime_type(field.value(), Some(expected_field))?;
            if actual_field != expected_field {
                return None;
            }
        }
        self.substitute_nominal_args(def_id, expected_args, &substitutions)
            .map(ComptimeValueType::Runtime)
    }

    fn structural_resolved_comptime_struct_literal_type(
        &mut self,
        fields: &[ResolvedComptimeFieldInit],
    ) -> Option<ComptimeValueType> {
        let mut seen = HashSet::new();
        let mut typed_fields = Vec::with_capacity(fields.len());
        for field in fields {
            if !seen.insert(field.name()) {
                return None;
            }
            typed_fields.push(ComptimeValueFieldType {
                name: field.name().to_string(),
                ty: self.resolved_comptime_expr_type(field.value(), None)?,
            });
        }
        Some(ComptimeValueType::Struct(typed_fields))
    }

    fn comptime_nominal_struct_field_type(
        &mut self,
        ty: InternedTyId,
        name: &str,
    ) -> Option<InternedTyId> {
        let (def_id, args) = self.expected_nominal_parts(ty)?;
        if self.def_kind_of(def_id) != Some(DefKind::Struct) {
            return None;
        }
        let signature = self.struct_signature_for(def_id)?;
        self.comptime_struct_field_types(&signature, &args)?
            .get(name)
            .copied()
    }

    fn resolved_comptime_struct_field_actual_type(
        &mut self,
        value: &ResolvedComptimeExpr,
        expected: InternedTyId,
    ) -> Option<InternedTyId> {
        self.resolved_comptime_arg_runtime_type(value, Some(expected))
            .filter(|ty| !self.type_contains_generic(*ty))
            .or_else(|| self.resolved_comptime_arg_runtime_type(value, None))
    }

    fn substitute_current_ty_generics(
        &mut self,
        ty: InternedTyId,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Option<InternedTyId> {
        let current_module = self.current_execution_module_id();
        let interner = self.working_interners.get_mut(&current_module)?;
        Some(substitute_ty_generics_in_interner(
            interner,
            ty,
            &|generic| substitutions.get(generic).copied(),
        ))
    }

    fn expected_nominal_parts(&self, ty: InternedTyId) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        match self.ty_kind(ty)? {
            TyKind::Nominal { def_id, args } => Some((def_id, args)),
            _ => None,
        }
    }

    fn struct_signature_for(
        &self,
        def_id: GlobalDefId,
    ) -> Option<nia_item_signatures::StructSignature> {
        self.signatures_for_module(def_id.module_id)?
            .structs
            .get(&def_id.def_id)
            .cloned()
    }

    fn comptime_struct_field_types(
        &mut self,
        signature: &nia_item_signatures::StructSignature,
        expected_args: &[InternedTyId],
    ) -> Option<HashMap<String, InternedTyId>> {
        if signature.generics.len() != expected_args.len() {
            return None;
        }
        let current_module = self.current_execution_module_id();
        let expected_args = expected_args
            .iter()
            .copied()
            .map(|arg| self.import_ty_into_module_or_none(arg, current_module))
            .collect::<Option<Vec<_>>>()?;
        let substitutions = signature
            .generics
            .iter()
            .cloned()
            .zip(expected_args)
            .collect::<HashMap<_, _>>();
        let mut fields = HashMap::new();
        for field in &signature.fields {
            let imported = self.import_ty_into_module_or_none(field.ty, current_module)?;
            let ty = {
                let interner = self.working_interners.get_mut(&current_module)?;
                substitute_ty_generics_in_interner(interner, imported, &|generic| {
                    substitutions.get(generic).copied()
                })
            };
            fields.insert(field.name.clone(), ty);
        }
        Some(fields)
    }

    fn substitute_nominal_args(
        &mut self,
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Option<InternedTyId> {
        let current_module = self.current_execution_module_id();
        let args = {
            let interner = self.working_interners.get_mut(&current_module)?;
            args.into_iter()
                .map(|arg| {
                    substitute_ty_generics_in_interner(interner, arg, &|generic| {
                        substitutions.get(generic).copied()
                    })
                })
                .collect()
        };
        self.working_interners
            .get_mut(&current_module)
            .map(|interner| interner.intern(TyKind::Nominal { def_id, args }))
    }

    fn resolved_comptime_switch_expr_type(
        &mut self,
        switch: &ResolvedComptimeSwitch,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
        let target_ty = self.resolved_comptime_arg_runtime_type(switch.target(), None);
        let expected = expected.and_then(|expected| self.usable_comptime_expected_type(expected));
        let mut result_ty = expected.map(ComptimeValueType::Runtime);
        let mut saw_value_arm = false;
        for arm in switch.arms() {
            let arm_ty = result_ty
                .clone()
                .and_then(|expected| {
                    let runtime_expected = expected.runtime();
                    let arm_ty =
                        self.resolved_comptime_switch_arm_type(arm, target_ty, runtime_expected)?;
                    (arm_ty == ComptimeArmType::Value(expected)).then_some(arm_ty)
                })
                .or_else(|| {
                    self.resolved_comptime_switch_arm_type(
                        arm,
                        target_ty,
                        result_ty.as_ref()?.runtime(),
                    )
                })
                .or_else(|| self.resolved_comptime_switch_arm_type(arm, target_ty, None))?;
            let ComptimeArmType::Value(arm_ty) = arm_ty else {
                continue;
            };
            saw_value_arm = true;
            match &result_ty {
                Some(result_ty) if *result_ty != arm_ty => return None,
                Some(_) => {}
                None => result_ty = Some(arm_ty),
            }
        }
        saw_value_arm.then_some(result_ty).flatten()
    }

    fn resolved_comptime_switch_arm_type(
        &mut self,
        arm: &nia_comptime_ir::ResolvedComptimeSwitchArm,
        target_ty: Option<InternedTyId>,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeArmType> {
        let target_ty = target_ty?;
        self.check_resolved_comptime_switch_patterns(arm.patterns(), target_ty)?;
        if !self.resolved_comptime_switch_arm_binds_pattern_locals(arm) {
            return self.resolved_comptime_switch_arm_body_type(arm.body(), expected);
        }
        self.push_typed_comptime_scope();
        let result = (|| {
            self.bind_typed_resolved_comptime_switch_patterns(arm.patterns(), target_ty)?;
            self.resolved_comptime_switch_arm_body_type(arm.body(), expected)
        })();
        self.pop_typed_comptime_scope();
        result
    }

    fn resolved_comptime_switch_arm_binds_pattern_locals(
        &self,
        arm: &nia_comptime_ir::ResolvedComptimeSwitchArm,
    ) -> bool {
        arm.patterns().iter().any(|pattern| {
            matches!(
                pattern.kind(),
                ResolvedComptimeSwitchPatternKind::OptionalSome { .. }
                    | ResolvedComptimeSwitchPatternKind::ErrorOk { .. }
                    | ResolvedComptimeSwitchPatternKind::ErrorErr { .. }
            )
        })
    }

    fn check_resolved_comptime_switch_patterns(
        &mut self,
        patterns: &[ResolvedComptimeSwitchPattern],
        target_ty: InternedTyId,
    ) -> Option<()> {
        for pattern in patterns {
            match pattern.kind() {
                ResolvedComptimeSwitchPatternKind::Default => {}
                ResolvedComptimeSwitchPatternKind::Expr(expr) => {
                    let target_ty = ComptimeValueType::Runtime(target_ty);
                    let pattern_ty = self
                        .resolved_comptime_expr_type(expr, Some(target_ty.runtime()?))
                        .or_else(|| self.resolved_comptime_expr_type(expr, None))?;
                    if pattern_ty != target_ty
                        && !self.comptime_equality_types_are_compatible(&target_ty, &pattern_ty)
                    {
                        return None;
                    }
                }
                ResolvedComptimeSwitchPatternKind::Range { start, end, .. } => {
                    if !self.is_integer_runtime_type(target_ty) {
                        return None;
                    }
                    let start_ty =
                        self.resolved_comptime_arg_runtime_type(start, Some(target_ty))?;
                    let end_ty = self.resolved_comptime_arg_runtime_type(end, Some(target_ty))?;
                    if start_ty != target_ty || end_ty != target_ty {
                        return None;
                    }
                }
                ResolvedComptimeSwitchPatternKind::OptionalSome { .. }
                | ResolvedComptimeSwitchPatternKind::OptionalNull { .. } => {
                    if !matches!(self.ty_kind(target_ty), Some(TyKind::Optional { .. })) {
                        return None;
                    }
                }
                ResolvedComptimeSwitchPatternKind::ErrorOk { .. }
                | ResolvedComptimeSwitchPatternKind::ErrorErr { .. } => {
                    if !matches!(self.ty_kind(target_ty), Some(TyKind::ErrorUnion { .. })) {
                        return None;
                    }
                }
            }
        }
        Some(())
    }

    fn bind_typed_resolved_comptime_switch_patterns(
        &mut self,
        patterns: &[ResolvedComptimeSwitchPattern],
        target_ty: InternedTyId,
    ) -> Option<()> {
        for pattern in patterns {
            let (local_id, ty) = match pattern.kind() {
                ResolvedComptimeSwitchPatternKind::OptionalSome { local_id, .. } => {
                    let Some(TyKind::Optional { elem }) = self.ty_kind(target_ty) else {
                        return None;
                    };
                    (*local_id, elem)
                }
                ResolvedComptimeSwitchPatternKind::ErrorOk { local_id, .. } => {
                    let Some(TyKind::ErrorUnion { value, .. }) = self.ty_kind(target_ty) else {
                        return None;
                    };
                    (*local_id, value)
                }
                ResolvedComptimeSwitchPatternKind::ErrorErr { local_id, .. } => {
                    let Some(TyKind::ErrorUnion { error, .. }) = self.ty_kind(target_ty) else {
                        return None;
                    };
                    (*local_id, error)
                }
                ResolvedComptimeSwitchPatternKind::Default
                | ResolvedComptimeSwitchPatternKind::OptionalNull { .. }
                | ResolvedComptimeSwitchPatternKind::Expr(_)
                | ResolvedComptimeSwitchPatternKind::Range { .. } => continue,
            };
            self.bind_comptime_local_type(local_id, ComptimeValueType::Runtime(ty));
        }
        Some(())
    }

    fn resolved_comptime_switch_arm_body_type(
        &mut self,
        body: &ResolvedComptimeSwitchArmBody,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeArmType> {
        match body.kind() {
            ResolvedComptimeSwitchArmBodyKind::Expr(expr) => self
                .resolved_comptime_expr_type(expr, expected)
                .map(ComptimeArmType::Value),
            ResolvedComptimeSwitchArmBodyKind::Block(block) => {
                self.resolved_comptime_switch_block_arm_type(block, expected)
            }
            ResolvedComptimeSwitchArmBodyKind::Stmt(stmt) => {
                self.resolved_comptime_stmt_arm_type(stmt, expected)
            }
        }
    }

    fn resolved_comptime_switch_block_arm_type(
        &mut self,
        block: &ResolvedComptimeBlock,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeArmType> {
        self.resolved_comptime_block_tail_type(block, expected)
            .map(ComptimeArmType::Value)
    }

    fn resolved_comptime_stmt_arm_type(
        &mut self,
        stmt: &nia_comptime_ir::ResolvedComptimeStmt,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeArmType> {
        match stmt.kind() {
            ResolvedComptimeStmtKind::Expr(expr) => self
                .resolved_comptime_expr_type(expr, expected)
                .map(ComptimeArmType::Value),
            ResolvedComptimeStmtKind::Return(_)
            | ResolvedComptimeStmtKind::Break
            | ResolvedComptimeStmtKind::Continue => Some(ComptimeArmType::ControlFlow),
            ResolvedComptimeStmtKind::Binding(_)
            | ResolvedComptimeStmtKind::If { .. }
            | ResolvedComptimeStmtKind::ForIn(_)
            | ResolvedComptimeStmtKind::While { .. }
            | ResolvedComptimeStmtKind::Loop { .. } => None,
        }
    }

    fn resolved_comptime_if_expr_type(
        &mut self,
        cond: &ResolvedComptimeExpr,
        then_branch: &ResolvedComptimeBlock,
        else_branch: Option<&ResolvedComptimeExpr>,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
        let bool_ty = self.current_runtime_primitive_type(PrimitiveTy::Bool);
        let cond_ty = self.resolved_comptime_arg_runtime_type(cond, Some(bool_ty))?;
        if cond_ty != bool_ty {
            return None;
        }
        let expected = expected.and_then(|expected| self.usable_comptime_expected_type(expected));
        let else_branch = else_branch?;
        if let Some(expected) = expected {
            let then_ty = self
                .resolved_comptime_block_tail_runtime_type(then_branch, Some(expected))
                .or_else(|| self.resolved_comptime_block_tail_runtime_type(then_branch, None))?;
            let else_ty = self
                .resolved_comptime_arg_runtime_type(else_branch, Some(expected))
                .filter(|else_ty| *else_ty == then_ty)
                .or_else(|| self.resolved_comptime_arg_runtime_type(else_branch, Some(then_ty)))?;
            return (then_ty == else_ty).then_some(ComptimeValueType::Runtime(then_ty));
        }
        let then_ty = self.resolved_comptime_block_tail_type(then_branch, None)?;
        let else_ty = self.resolved_comptime_expr_type(else_branch, None)?;
        (then_ty == else_ty).then_some(then_ty)
    }

    fn usable_comptime_expected_type(&self, ty: InternedTyId) -> Option<InternedTyId> {
        match self.ty_kind(ty) {
            Some(TyKind::GenericParam(_)) => None,
            _ => Some(ty),
        }
    }

    fn resolved_comptime_block_tail_runtime_type(
        &mut self,
        block: &ResolvedComptimeBlock,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        self.resolved_comptime_block_tail_type(block, expected)
            .and_then(|ty| ty.runtime())
    }

    fn resolved_comptime_block_tail_type(
        &mut self,
        block: &ResolvedComptimeBlock,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
        if block.is_empty() {
            return self.resolved_comptime_expr_type(block.tail()?, expected);
        }
        self.push_typed_comptime_scope();
        let result = (|| {
            for stmt in block.stmts() {
                self.bind_typed_resolved_comptime_stmt(stmt)?;
            }
            self.resolved_comptime_expr_type(block.tail()?, expected)
        })();
        self.pop_typed_comptime_scope();
        result
    }

    fn bind_typed_resolved_comptime_stmt(
        &mut self,
        stmt: &nia_comptime_ir::ResolvedComptimeStmt,
    ) -> Option<()> {
        match stmt.kind() {
            ResolvedComptimeStmtKind::Binding(binding) => {
                let ty = binding
                    .explicit_type()
                    .map(|ty| self.substitute_ty_generics(ty))
                    .map(ComptimeValueType::Runtime)
                    .or_else(|| self.resolved_comptime_expr_type(binding.value(), None))?;
                self.bind_comptime_local_type(binding.local_id(), ty);
                Some(())
            }
            ResolvedComptimeStmtKind::Expr(_) => Some(()),
            ResolvedComptimeStmtKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.check_resolved_comptime_bool_condition(cond)?;
                self.check_resolved_comptime_block(then_branch)?;
                if let Some(else_branch) = else_branch {
                    self.check_resolved_comptime_block(else_branch)?;
                }
                Some(())
            }
            ResolvedComptimeStmtKind::ForIn(for_in) => {
                self.check_resolved_comptime_for_in_stmt(for_in)
            }
            ResolvedComptimeStmtKind::While { cond, body } => {
                self.check_resolved_comptime_bool_condition(cond)?;
                self.check_resolved_comptime_block(body)
            }
            ResolvedComptimeStmtKind::Loop { body } => self.check_resolved_comptime_block(body),
            ResolvedComptimeStmtKind::Return(_)
            | ResolvedComptimeStmtKind::Break
            | ResolvedComptimeStmtKind::Continue => None,
        }
    }

    fn check_resolved_comptime_block(&mut self, block: &ResolvedComptimeBlock) -> Option<()> {
        self.push_typed_comptime_scope();
        let result = (|| {
            for stmt in block.stmts() {
                self.check_resolved_comptime_stmt(stmt)?;
            }
            if let Some(tail) = block.tail() {
                self.resolved_comptime_expr_type(tail, None)?;
            }
            Some(())
        })();
        self.pop_typed_comptime_scope();
        result
    }

    fn check_resolved_comptime_stmt(
        &mut self,
        stmt: &nia_comptime_ir::ResolvedComptimeStmt,
    ) -> Option<()> {
        match stmt.kind() {
            ResolvedComptimeStmtKind::Binding(_)
            | ResolvedComptimeStmtKind::If { .. }
            | ResolvedComptimeStmtKind::ForIn(_)
            | ResolvedComptimeStmtKind::While { .. }
            | ResolvedComptimeStmtKind::Loop { .. } => self.bind_typed_resolved_comptime_stmt(stmt),
            ResolvedComptimeStmtKind::Expr(_)
            | ResolvedComptimeStmtKind::Break
            | ResolvedComptimeStmtKind::Continue => Some(()),
            ResolvedComptimeStmtKind::Return(Some(expr)) => {
                self.resolved_comptime_expr_type(expr, None)?;
                Some(())
            }
            ResolvedComptimeStmtKind::Return(None) => Some(()),
        }
    }

    fn check_resolved_comptime_for_in_stmt(
        &mut self,
        for_in: &nia_comptime_ir::ResolvedComptimeForIn,
    ) -> Option<()> {
        let iter_ty = self.resolved_comptime_expr_type(for_in.iter(), None)?;
        let binding_ty = self.comptime_for_in_binding_type(iter_ty)?;
        self.push_typed_comptime_scope();
        let result = (|| {
            self.bind_comptime_local_type(for_in.binding().local_id(), binding_ty);
            for stmt in for_in.body().stmts() {
                self.check_resolved_comptime_stmt(stmt)?;
            }
            if let Some(tail) = for_in.body().tail() {
                self.resolved_comptime_expr_type(tail, None)?;
            }
            Some(())
        })();
        self.pop_typed_comptime_scope();
        result
    }

    fn comptime_for_in_binding_type(
        &mut self,
        iter_ty: ComptimeValueType,
    ) -> Option<ComptimeValueType> {
        match iter_ty {
            ComptimeValueType::Array { elem, .. } => Some(*elem),
            ComptimeValueType::Runtime(ty) => match self.ty_kind(ty)? {
                TyKind::Range {
                    bound: Some(bound), ..
                } => {
                    let bound = self
                        .import_ty_into_module_or_none(bound, self.current_execution_module_id())?;
                    Some(ComptimeValueType::Runtime(bound))
                }
                _ => None,
            },
            ComptimeValueType::Struct(_)
            | ComptimeValueType::Int
            | ComptimeValueType::Bool
            | ComptimeValueType::String => None,
        }
    }

    fn check_resolved_comptime_bool_condition(
        &mut self,
        cond: &ResolvedComptimeExpr,
    ) -> Option<()> {
        let bool_ty = self.current_runtime_primitive_type(PrimitiveTy::Bool);
        let cond_ty = self.resolved_comptime_arg_runtime_type(cond, Some(bool_ty))?;
        (cond_ty == bool_ty).then_some(())
    }

    fn push_typed_comptime_scope(&mut self) {
        self.call_locals.push(ComptimeCallFrame::default());
    }

    fn pop_typed_comptime_scope(&mut self) {
        self.call_locals.pop();
    }

    fn bind_comptime_local_type(&mut self, local_id: LocalId, ty: ComptimeValueType) {
        let Some(frame) = self.call_locals.last_mut() else {
            return;
        };
        frame.local_types.insert(local_id, ty);
    }

    fn resolved_comptime_unary_expr_type(
        &mut self,
        op: ComptimeUnaryOp,
        inner: &ResolvedComptimeExpr,
    ) -> Option<ComptimeValueType> {
        match op {
            ComptimeUnaryOp::Not => {
                let bool_ty = self.current_runtime_primitive_type(PrimitiveTy::Bool);
                let inner_ty = self.resolved_comptime_arg_runtime_type(inner, Some(bool_ty))?;
                (inner_ty == bool_ty).then_some(ComptimeValueType::Runtime(bool_ty))
            }
            ComptimeUnaryOp::Neg => {
                let inner_ty = self.resolved_comptime_arg_runtime_type(inner, None)?;
                (self.is_integer_runtime_type(inner_ty) || self.is_float_runtime_type(inner_ty))
                    .then_some(ComptimeValueType::Runtime(inner_ty))
            }
            ComptimeUnaryOp::BitNot => {
                let inner_ty = self.resolved_comptime_arg_runtime_type(inner, None)?;
                self.is_integer_runtime_type(inner_ty)
                    .then_some(ComptimeValueType::Runtime(inner_ty))
            }
            ComptimeUnaryOp::RefReadOnly | ComptimeUnaryOp::Ref | ComptimeUnaryOp::Deref => None,
        }
    }

    fn resolved_comptime_range_type(
        &mut self,
        range: &nia_comptime_ir::ResolvedComptimeRange,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
        let kind = match (
            range.start().is_some(),
            range.end().is_some(),
            range.is_inclusive(),
        ) {
            (true, true, false) => RangeTyKind::Exclusive,
            (true, true, true) => RangeTyKind::Inclusive,
            (true, false, false) => RangeTyKind::From,
            (false, true, false) => RangeTyKind::To,
            (false, true, true) => RangeTyKind::ToInclusive,
            (false, false, false) => RangeTyKind::Full,
            (true, false, true) | (false, false, true) => return None,
        };
        let expected_bound = expected.and_then(|expected| match self.ty_kind(expected) {
            Some(TyKind::Range {
                kind: expected_kind,
                bound,
            }) if expected_kind == kind => bound,
            _ => None,
        });
        let start_ty = match range.start() {
            Some(start) => Some(self.resolved_comptime_arg_runtime_type(start, expected_bound)?),
            None => None,
        };
        let end_ty = match range.end() {
            Some(end) => {
                Some(self.resolved_comptime_arg_runtime_type(end, expected_bound.or(start_ty))?)
            }
            None => None,
        };
        let bound = match (start_ty, end_ty) {
            (Some(start_ty), Some(end_ty)) if start_ty == end_ty => Some(start_ty),
            (Some(bound), None) | (None, Some(bound)) => Some(bound),
            (None, None) => expected_bound,
            (Some(_), Some(_)) => return None,
        };
        let module_id = self.current_execution_module_id();
        let bound = match bound {
            Some(bound) => Some(self.import_ty_into_module_or_none(bound, module_id)?),
            None => None,
        };
        let ty = self
            .working_interners
            .get_mut(&module_id)?
            .intern(TyKind::Range { kind, bound });
        Some(ComptimeValueType::Runtime(ty))
    }

    fn resolved_comptime_binary_expr_type(
        &mut self,
        lhs: &ResolvedComptimeExpr,
        op: ComptimeBinaryOp,
        rhs: &ResolvedComptimeExpr,
    ) -> Option<ComptimeValueType> {
        match op {
            ComptimeBinaryOp::And | ComptimeBinaryOp::Or => {
                self.resolved_comptime_bool_binary_expr_type(lhs, rhs)
            }
            ComptimeBinaryOp::Eq | ComptimeBinaryOp::Ne => {
                self.resolved_comptime_equality_expr_type(lhs, rhs)
            }
            ComptimeBinaryOp::Lt
            | ComptimeBinaryOp::Le
            | ComptimeBinaryOp::Gt
            | ComptimeBinaryOp::Ge => {
                let lhs_ty = self.resolved_comptime_arg_runtime_type(lhs, None)?;
                let rhs_ty = self.resolved_comptime_arg_runtime_type(rhs, Some(lhs_ty))?;
                (lhs_ty == rhs_ty
                    && (self.is_integer_runtime_type(lhs_ty) || self.is_float_runtime_type(lhs_ty)))
                .then_some(ComptimeValueType::Runtime(
                    self.current_runtime_primitive_type(PrimitiveTy::Bool),
                ))
            }
            ComptimeBinaryOp::Mul
            | ComptimeBinaryOp::Div
            | ComptimeBinaryOp::Rem
            | ComptimeBinaryOp::Add
            | ComptimeBinaryOp::Sub
            | ComptimeBinaryOp::Shl
            | ComptimeBinaryOp::Shr
            | ComptimeBinaryOp::BitAnd
            | ComptimeBinaryOp::BitXor
            | ComptimeBinaryOp::BitOr => {
                let lhs_ty = self.resolved_comptime_arg_runtime_type(lhs, None)?;
                let rhs_ty = self.resolved_comptime_arg_runtime_type(rhs, Some(lhs_ty))?;
                let allowed = match op {
                    ComptimeBinaryOp::Shl
                    | ComptimeBinaryOp::Shr
                    | ComptimeBinaryOp::BitAnd
                    | ComptimeBinaryOp::BitXor
                    | ComptimeBinaryOp::BitOr => self.is_integer_runtime_type(lhs_ty),
                    _ => self.is_integer_runtime_type(lhs_ty) || self.is_float_runtime_type(lhs_ty),
                };
                (lhs_ty == rhs_ty && allowed).then_some(ComptimeValueType::Runtime(lhs_ty))
            }
        }
    }

    fn resolved_comptime_cast_type(
        &mut self,
        expr: &ResolvedComptimeExpr,
        target: InternedTyId,
    ) -> Option<ComptimeValueType> {
        let source = self.resolved_comptime_arg_runtime_type(expr, None)?;
        let target =
            self.import_ty_into_module_or_none(target, self.current_execution_module_id())?;
        self.comptime_runtime_cast_is_supported(source, target)
            .then_some(ComptimeValueType::Runtime(target))
    }

    fn comptime_runtime_cast_is_supported(
        &mut self,
        source: InternedTyId,
        target: InternedTyId,
    ) -> bool {
        let Some(TyKind::Primitive(source)) = self.ty_kind(source) else {
            return false;
        };
        let Some(TyKind::Primitive(target)) = self.ty_kind(target) else {
            return false;
        };
        let source_numeric = primitive_integer_layout(source, self.input.target.pointer_width)
            .is_some()
            || is_float_primitive(source);
        let target_numeric = primitive_integer_layout(target, self.input.target.pointer_width)
            .is_some()
            || is_float_primitive(target);
        source_numeric && target_numeric
    }

    fn resolved_comptime_bool_binary_expr_type(
        &mut self,
        lhs: &ResolvedComptimeExpr,
        rhs: &ResolvedComptimeExpr,
    ) -> Option<ComptimeValueType> {
        let bool_ty = self.current_runtime_primitive_type(PrimitiveTy::Bool);
        let lhs_ty = self.resolved_comptime_arg_runtime_type(lhs, Some(bool_ty))?;
        let rhs_ty = self.resolved_comptime_arg_runtime_type(rhs, Some(bool_ty))?;
        (lhs_ty == bool_ty && rhs_ty == bool_ty).then_some(ComptimeValueType::Runtime(bool_ty))
    }

    fn resolved_comptime_equality_expr_type(
        &mut self,
        lhs: &ResolvedComptimeExpr,
        rhs: &ResolvedComptimeExpr,
    ) -> Option<ComptimeValueType> {
        let lhs_ty = self.resolved_comptime_expr_type(lhs, None)?;
        let rhs_ty = self
            .resolved_comptime_expr_type(rhs, lhs_ty.runtime())
            .or_else(|| self.resolved_comptime_expr_type(rhs, None))?;
        (lhs_ty == rhs_ty || self.comptime_equality_types_are_compatible(&lhs_ty, &rhs_ty))
            .then_some(ComptimeValueType::Runtime(
                self.current_runtime_primitive_type(PrimitiveTy::Bool),
            ))
    }

    fn comptime_equality_types_are_compatible(
        &mut self,
        lhs: &ComptimeValueType,
        rhs: &ComptimeValueType,
    ) -> bool {
        (matches!(lhs, ComptimeValueType::String) && self.is_runtime_char_array_type(rhs))
            || (matches!(rhs, ComptimeValueType::String) && self.is_runtime_char_array_type(lhs))
    }

    fn is_runtime_char_array_type(&self, ty: &ComptimeValueType) -> bool {
        let ComptimeValueType::Runtime(ty) = ty else {
            return false;
        };
        let Some(TyKind::Array { elem, .. }) = self.ty_kind(*ty) else {
            return false;
        };
        matches!(
            self.ty_kind(elem),
            Some(TyKind::Primitive(PrimitiveTy::Char))
        )
    }

    fn current_runtime_primitive_type(&self, primitive: PrimitiveTy) -> InternedTyId {
        self.source_interner_for_module(self.current_execution_module_id())
            .unwrap_or(self.input.interner)
            .primitive(primitive)
    }

    fn comptime_string_literal_type(
        &mut self,
        literal: &ComptimeStringLiteral,
    ) -> Option<ComptimeValueType> {
        let len = nia_comptime_engine::eval_string_literal(literal)?
            .chars()
            .count() as u64;
        self.comptime_array_runtime_type(len, PrimitiveTy::Char)
            .map(ComptimeValueType::Runtime)
    }

    fn comptime_byte_string_literal_type(&mut self, len: u64) -> Option<ComptimeValueType> {
        self.comptime_array_runtime_type(len, PrimitiveTy::U8)
            .map(ComptimeValueType::Runtime)
    }

    fn comptime_array_runtime_type(
        &mut self,
        len: u64,
        elem_primitive: PrimitiveTy,
    ) -> Option<InternedTyId> {
        self.comptime_runtime_type(
            self.current_runtime_primitive_type(elem_primitive),
            |elem| TyKind::Array {
                len: ArrayLenTy::ConstValue(len),
                elem,
            },
            self.current_execution_module_id(),
        )
    }

    fn is_integer_runtime_type(&self, ty: InternedTyId) -> bool {
        matches!(
            self.ty_kind(ty),
            Some(TyKind::Primitive(
                PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::I128
                    | PrimitiveTy::Isize
                    | PrimitiveTy::U8
                    | PrimitiveTy::U16
                    | PrimitiveTy::U32
                    | PrimitiveTy::U64
                    | PrimitiveTy::U128
                    | PrimitiveTy::Usize
            ))
        )
    }

    fn is_float_runtime_type(&self, ty: InternedTyId) -> bool {
        matches!(
            self.ty_kind(ty),
            Some(TyKind::Primitive(PrimitiveTy::F32 | PrimitiveTy::F64))
        )
    }

    fn resolved_comptime_call_return_type(
        &mut self,
        span: Span,
        callee: &ResolvedComptimeExpr,
        type_args: &[ResolvedComptimeTypeArg],
        args: &[ResolvedComptimeExpr],
    ) -> Option<InternedTyId> {
        let function_id = self.resolved_comptime_function(callee)?;
        let signature = self
            .signatures_for_module(function_id.module_id)?
            .functions
            .get(&function_id.def_id)?
            .clone();
        let substitutions = self
            .instantiate_resolved_function_generics(
                span,
                function_id.module_id,
                &signature,
                type_args,
                args,
            )
            .ok()?;
        self.substitute_ty_into_current_module(
            function_id.module_id,
            signature.return_type,
            &substitutions,
        )
    }

    fn substitute_ty_into_current_module(
        &mut self,
        source_module_id: ModuleId,
        ty: InternedTyId,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Option<InternedTyId> {
        self.ensure_working_interner(source_module_id)?;
        let substituted = {
            let interner = self.working_interners.get_mut(&source_module_id)?;
            substitute_ty_generics_in_interner(interner, ty, &|generic| {
                substitutions.get(generic).copied()
            })
        };
        self.import_ty_into_module_or_none(substituted, self.current_execution_module_id())
    }

    fn expected_error_union_parts(
        &self,
        expected: InternedTyId,
    ) -> Option<(InternedTyId, InternedTyId)> {
        match self.ty_kind(expected) {
            Some(TyKind::ErrorUnion { error, value }) => Some((error, value)),
            _ => None,
        }
    }

    fn comptime_runtime_type(
        &mut self,
        elem: InternedTyId,
        kind: impl FnOnce(InternedTyId) -> TyKind,
        target_module_id: ModuleId,
    ) -> Option<InternedTyId> {
        let imported_elem = self.import_ty_into_module_or_none(elem, target_module_id)?;
        self.working_interners
            .get_mut(&target_module_id)
            .map(|interner| interner.intern(kind(imported_elem)))
    }

    fn comptime_error_union_type(
        &mut self,
        error: InternedTyId,
        value: InternedTyId,
    ) -> Option<InternedTyId> {
        let target_module_id = self.current_execution_module_id();
        let error = self.import_ty_into_module_or_none(error, target_module_id)?;
        let value = self.import_ty_into_module_or_none(value, target_module_id)?;
        self.working_interners
            .get_mut(&target_module_id)
            .map(|interner| interner.intern(TyKind::ErrorUnion { error, value }))
    }

    fn call_local_type(&self, local_id: LocalId) -> Option<ComptimeValueType> {
        self.call_locals
            .iter()
            .rev()
            .find_map(|frame| frame.local_types.get(&local_id).cloned())
    }

    fn builtin_comptime_type(&self) -> ComptimeValueType {
        builtin_comptime_value_type(self.current_runtime_primitive_type(PrimitiveTy::Usize))
    }

    fn infer_generics_from_tys(
        &mut self,
        span: Span,
        target_module_id: ModuleId,
        pattern_ty: InternedTyId,
        actual_ty: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
    ) -> Result<(), ComptimeError> {
        let Some(pattern_kind) = self
            .source_interner_for_module(pattern_ty.interner_id)
            .and_then(|interner| interner.get(pattern_ty))
            .cloned()
        else {
            return Ok(());
        };
        match pattern_kind {
            TyKind::GenericParam(name) => {
                let imported = self.import_ty_into_module(span, actual_ty, target_module_id)?;
                if let Some(existing) = substitutions.get(&name) {
                    if *existing != imported {
                        return Err(ComptimeError {
                            span,
                            message: format!(
                                "conflicting inferred comptime generic type argument `{name}`"
                            ),
                        });
                    }
                } else {
                    substitutions.insert(name, imported);
                }
            }
            TyKind::Pointer { is_readonly, elem } => {
                if let Some(TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }) = self.ty_kind(actual_ty)
                    && is_readonly == actual_readonly
                {
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        elem,
                        actual_elem,
                        substitutions,
                    )?;
                }
            }
            TyKind::Slice { is_readonly, elem } => {
                if let Some(TyKind::Slice {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }) = self.ty_kind(actual_ty)
                    && is_readonly == actual_readonly
                {
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        elem,
                        actual_elem,
                        substitutions,
                    )?;
                }
            }
            TyKind::Array { len, elem } => {
                if let Some(TyKind::Array {
                    len: actual_len,
                    elem: actual_elem,
                }) = self.ty_kind(actual_ty)
                    && len == actual_len
                {
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        elem,
                        actual_elem,
                        substitutions,
                    )?;
                }
            }
            TyKind::Range { kind, bound } => {
                if let Some(TyKind::Range {
                    kind: actual_kind,
                    bound: actual_bound,
                }) = self.ty_kind(actual_ty)
                    && kind == actual_kind
                    && let (Some(bound), Some(actual_bound)) = (bound, actual_bound)
                {
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        bound,
                        actual_bound,
                        substitutions,
                    )?;
                }
            }
            TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            } => {
                if let Some(TyKind::FunctionPointer {
                    params: actual_params,
                    return_type: actual_return_type,
                    is_variadic: actual_is_variadic,
                }) = self.ty_kind(actual_ty)
                    && is_variadic == actual_is_variadic
                    && params.len() == actual_params.len()
                {
                    for (param, actual_param) in params.into_iter().zip(actual_params) {
                        self.infer_generics_from_tys(
                            span,
                            target_module_id,
                            param,
                            actual_param,
                            substitutions,
                        )?;
                    }
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        return_type,
                        actual_return_type,
                        substitutions,
                    )?;
                }
            }
            TyKind::Optional { elem } => {
                if let Some(TyKind::Optional { elem: actual_elem }) = self.ty_kind(actual_ty) {
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        elem,
                        actual_elem,
                        substitutions,
                    )?;
                }
            }
            TyKind::ErrorUnion { error, value } => {
                if let Some(TyKind::ErrorUnion {
                    error: actual_error,
                    value: actual_value,
                }) = self.ty_kind(actual_ty)
                {
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        error,
                        actual_error,
                        substitutions,
                    )?;
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        value,
                        actual_value,
                        substitutions,
                    )?;
                }
            }
            TyKind::Nominal { def_id, args } => {
                if let Some(TyKind::Nominal {
                    def_id: actual_def_id,
                    args: actual_args,
                }) = self.ty_kind(actual_ty)
                    && def_id == actual_def_id
                    && args.len() == actual_args.len()
                {
                    for (arg, actual_arg) in args.into_iter().zip(actual_args) {
                        self.infer_generics_from_tys(
                            span,
                            target_module_id,
                            arg,
                            actual_arg,
                            substitutions,
                        )?;
                    }
                }
            }
            TyKind::BuiltinTrait { args, .. } => {
                if let Some(TyKind::BuiltinTrait {
                    args: actual_args, ..
                }) = self.ty_kind(actual_ty)
                    && args.len() == actual_args.len()
                {
                    for (arg, actual_arg) in args.into_iter().zip(actual_args) {
                        self.infer_generics_from_tys(
                            span,
                            target_module_id,
                            arg,
                            actual_arg,
                            substitutions,
                        )?;
                    }
                }
            }
            TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            } => {
                if let Some(TyKind::TraitObject {
                    trait_args: actual_trait_args,
                    associated_type_bindings: actual_bindings,
                    ..
                }) = self.ty_kind(actual_ty)
                    && trait_args.len() == actual_trait_args.len()
                    && associated_type_bindings.len() == actual_bindings.len()
                {
                    for (arg, actual_arg) in trait_args.into_iter().zip(actual_trait_args) {
                        self.infer_generics_from_tys(
                            span,
                            target_module_id,
                            arg,
                            actual_arg,
                            substitutions,
                        )?;
                    }
                    for (binding, actual_binding) in
                        associated_type_bindings.into_iter().zip(actual_bindings)
                    {
                        if binding.name == actual_binding.name {
                            self.infer_generics_from_tys(
                                span,
                                target_module_id,
                                binding.ty,
                                actual_binding.ty,
                                substitutions,
                            )?;
                        }
                    }
                }
            }
            TyKind::Projection {
                self_ty,
                trait_args,
                ..
            } => {
                if let Some(TyKind::Projection {
                    self_ty: actual_self_ty,
                    trait_args: actual_trait_args,
                    ..
                }) = self.ty_kind(actual_ty)
                    && trait_args.len() == actual_trait_args.len()
                {
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        self_ty,
                        actual_self_ty,
                        substitutions,
                    )?;
                    for (arg, actual_arg) in trait_args.into_iter().zip(actual_trait_args) {
                        self.infer_generics_from_tys(
                            span,
                            target_module_id,
                            arg,
                            actual_arg,
                            substitutions,
                        )?;
                    }
                }
            }
            TyKind::Error | TyKind::ComptimeOnly | TyKind::Primitive(_) => {}
        }
        Ok(())
    }

    fn ty_kind(&self, ty: InternedTyId) -> Option<TyKind> {
        self.working_interners
            .get(&ty.interner_id)
            .and_then(|interner| interner.get(ty).cloned())
            .or_else(|| {
                self.source_interner_for_module(ty.interner_id)
                    .and_then(|interner| interner.get(ty).cloned())
            })
    }

    fn import_ty_into_module(
        &mut self,
        span: Span,
        ty: InternedTyId,
        target_module_id: ModuleId,
    ) -> Result<InternedTyId, ComptimeError> {
        let Some(source_interner) = self
            .working_interners
            .get(&ty.interner_id)
            .cloned()
            .or_else(|| self.source_interner_for_module(ty.interner_id).cloned())
        else {
            return Err(ComptimeError {
                span,
                message: "cannot resolve comptime generic function type argument interner"
                    .to_string(),
            });
        };
        let target = self
            .working_interners
            .get_mut(&target_module_id)
            .expect("target working interner must exist");
        Ok(import_type_into(target, &source_interner, ty))
    }

    fn import_ty_into_module_or_none(
        &mut self,
        ty: InternedTyId,
        target_module_id: ModuleId,
    ) -> Option<InternedTyId> {
        if ty.interner_id == target_module_id {
            return Some(ty);
        }
        let source_interner = self
            .working_interners
            .get(&ty.interner_id)
            .cloned()
            .or_else(|| self.source_interner_for_module(ty.interner_id).cloned())?;
        let target = self.working_interners.get_mut(&target_module_id)?;
        Some(import_type_into(target, &source_interner, ty))
    }
}

fn substitute_ty_generics_in_interner(
    interner: &mut TyInterner,
    ty: InternedTyId,
    lookup: &impl Fn(&str) -> Option<InternedTyId>,
) -> InternedTyId {
    match interner.get(ty).cloned() {
        Some(TyKind::GenericParam(name)) => lookup(&name).unwrap_or(ty),
        Some(TyKind::Pointer { is_readonly, elem }) => {
            let elem = substitute_ty_generics_in_interner(interner, elem, lookup);
            interner.intern(TyKind::Pointer { is_readonly, elem })
        }
        Some(TyKind::Slice { is_readonly, elem }) => {
            let elem = substitute_ty_generics_in_interner(interner, elem, lookup);
            interner.intern(TyKind::Slice { is_readonly, elem })
        }
        Some(TyKind::Array { len, elem }) => {
            let elem = substitute_ty_generics_in_interner(interner, elem, lookup);
            interner.intern(TyKind::Array { len, elem })
        }
        Some(TyKind::Range { kind, bound }) => {
            let bound =
                bound.map(|bound| substitute_ty_generics_in_interner(interner, bound, lookup));
            interner.intern(TyKind::Range { kind, bound })
        }
        Some(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        }) => {
            let params = params
                .into_iter()
                .map(|param| substitute_ty_generics_in_interner(interner, param, lookup))
                .collect();
            let return_type = substitute_ty_generics_in_interner(interner, return_type, lookup);
            interner.intern(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            })
        }
        Some(TyKind::Optional { elem }) => {
            let elem = substitute_ty_generics_in_interner(interner, elem, lookup);
            interner.intern(TyKind::Optional { elem })
        }
        Some(TyKind::ErrorUnion { error, value }) => {
            let error = substitute_ty_generics_in_interner(interner, error, lookup);
            let value = substitute_ty_generics_in_interner(interner, value, lookup);
            interner.intern(TyKind::ErrorUnion { error, value })
        }
        Some(TyKind::Nominal { def_id, args }) => {
            let args = args
                .into_iter()
                .map(|arg| substitute_ty_generics_in_interner(interner, arg, lookup))
                .collect();
            interner.intern(TyKind::Nominal { def_id, args })
        }
        Some(TyKind::BuiltinTrait { trait_id, args }) => {
            let args = args
                .into_iter()
                .map(|arg| substitute_ty_generics_in_interner(interner, arg, lookup))
                .collect();
            interner.intern(TyKind::BuiltinTrait { trait_id, args })
        }
        Some(TyKind::TraitObject {
            is_readonly,
            trait_id,
            trait_args,
            associated_type_bindings,
        }) => {
            let trait_args = trait_args
                .into_iter()
                .map(|arg| substitute_ty_generics_in_interner(interner, arg, lookup))
                .collect();
            let associated_type_bindings = associated_type_bindings
                .into_iter()
                .map(|binding| nia_ty::AssociatedTypeBindingTy {
                    trait_id: binding.trait_id,
                    trait_args: binding
                        .trait_args
                        .into_iter()
                        .map(|arg| substitute_ty_generics_in_interner(interner, arg, lookup))
                        .collect(),
                    name: binding.name,
                    ty: substitute_ty_generics_in_interner(interner, binding.ty, lookup),
                })
                .collect();
            interner.intern(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                associated_type_bindings,
            })
        }
        Some(TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            name,
        }) => {
            let self_ty = substitute_ty_generics_in_interner(interner, self_ty, lookup);
            let trait_args = trait_args
                .into_iter()
                .map(|arg| substitute_ty_generics_in_interner(interner, arg, lookup))
                .collect();
            interner.intern(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            })
        }
        Some(TyKind::Error | TyKind::ComptimeOnly | TyKind::Primitive(_)) | None => ty,
    }
}

impl ComptimeCommonEnv for Analyzer<'_> {
    fn resolve_builtin_value(
        &mut self,
        span: Span,
        builtin: ValueBuiltin,
    ) -> Result<ComptimeValue, ComptimeError> {
        let _ = span;
        match builtin {
            ValueBuiltin::Builtin => {
                Ok(nia_target_config::builtin_comptime_value(self.input.target))
            }
        }
    }

    fn cast_value(
        &mut self,
        span: Span,
        value: ComptimeValue,
        ty: InternedTyId,
    ) -> Result<ComptimeValue, ComptimeError> {
        let ty = self.substitute_ty_generics(ty);
        let Some(TyKind::Primitive(primitive)) = self.ty_kind(ty) else {
            return Ok(value);
        };
        let value = match value {
            ComptimeValue::Int(value) => {
                if is_float_primitive(primitive) {
                    let Some(value) = cast_int_to_float(value, primitive) else {
                        return Err(ComptimeError {
                            span,
                            message: format!(
                                "comptime cast result cannot be represented as `{}`",
                                primitive_ty_name(primitive)
                            ),
                        });
                    };
                    ComptimeValue::Float(value)
                } else {
                    let Some(value) =
                        cast_comptime_integer(value, primitive, self.input.target.pointer_width)
                    else {
                        return Err(ComptimeError {
                            span,
                            message: format!(
                                "comptime cast result cannot be represented as `{}`",
                                primitive_ty_name(primitive)
                            ),
                        });
                    };
                    ComptimeValue::Int(value)
                }
            }
            ComptimeValue::Float(value) => {
                if is_float_primitive(primitive) {
                    let Some(value) = cast_float_to_float(value, primitive) else {
                        return Err(ComptimeError {
                            span,
                            message: format!(
                                "comptime cast result cannot be represented as `{}`",
                                primitive_ty_name(primitive)
                            ),
                        });
                    };
                    ComptimeValue::Float(value)
                } else if primitive_integer_layout(primitive, self.input.target.pointer_width)
                    .is_some()
                {
                    let Some(value) =
                        cast_float_to_integer(value, primitive, self.input.target.pointer_width)
                    else {
                        return Err(ComptimeError {
                            span,
                            message: format!(
                                "comptime cast result cannot be represented as `{}`",
                                primitive_ty_name(primitive)
                            ),
                        });
                    };
                    ComptimeValue::Int(value)
                } else {
                    ComptimeValue::Float(value)
                }
            }
            value => value,
        };
        Ok(value)
    }

    fn push_comptime_scope(&mut self, _span: Span) -> Result<(), ComptimeError> {
        self.call_locals.push(ComptimeCallFrame::default());
        Ok(())
    }

    fn pop_comptime_scope(&mut self) {
        self.call_locals.pop();
    }

    fn bind_function_context(
        &mut self,
        span: Span,
        module_id: ModuleId,
        substitutions: Vec<(String, InternedTyId)>,
    ) -> Result<(), ComptimeError> {
        let Some(frame) = self.call_locals.last_mut() else {
            return Err(ComptimeError {
                span,
                message: "failed to bind comptime function type substitutions".to_string(),
            });
        };
        frame.module_id = Some(module_id);
        frame.type_substitutions.extend(substitutions);
        Ok(())
    }
}

impl ResolvedComptimeEnv for Analyzer<'_> {
    fn resolve_resolved_name(
        &mut self,
        span: Span,
        resolution: ComptimeNameResolution,
    ) -> Result<ComptimeValue, ComptimeError> {
        match resolution {
            ComptimeNameResolution::Local(local_id) => {
                if let Some(value) = self.call_local_value(local_id) {
                    return Ok(value);
                }
                self.eval_key(ComptimeKey::Local(local_id), span)
                    .ok_or_else(|| ComptimeError {
                        span,
                        message: "failed to evaluate resolved comptime local".to_string(),
                    })
            }
            ComptimeNameResolution::Global(global_id) => {
                if self.def_kind_of(global_id) == Some(DefKind::Comptime) {
                    return self
                        .eval_key(ComptimeKey::Global(global_id), span)
                        .ok_or_else(|| ComptimeError {
                            span,
                            message: "failed to evaluate resolved comptime global".to_string(),
                        });
                }
                Err(ComptimeError {
                    span,
                    message: "resolved comptime expression can only use comptime bindings"
                        .to_string(),
                })
            }
        }
    }

    fn resolve_resolved_layout_builtin(
        &mut self,
        span: Span,
        builtin: LayoutBuiltin,
        type_arg: &ResolvedComptimeTypeArg,
    ) -> Result<ComptimeValue, ComptimeError> {
        let module_id = self.current_execution_module_id();
        let ty_id = (|| {
            self.ensure_working_interner(module_id)?;
            self.import_ty_into_module_or_none(type_arg.ty(), module_id)
        })()
        .map(|ty| self.substitute_ty_generics(ty));
        let Some(ty_id) = ty_id else {
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

    fn call_resolved_function(
        &mut self,
        span: Span,
        callee: &ResolvedComptimeExpr,
        type_args: &[ResolvedComptimeTypeArg],
        arg_exprs: &[ResolvedComptimeExpr],
        args: Vec<ComptimeValue>,
    ) -> Result<ComptimeValue, ComptimeError> {
        let Some(function_id) = self.resolved_comptime_function(callee) else {
            return Err(ComptimeError {
                span,
                message: "comptime expression can only call `comptime fn`".to_string(),
            });
        };
        let Some(signature) = self
            .signatures_for_module(function_id.module_id)
            .and_then(|signatures| signatures.functions.get(&function_id.def_id))
            .cloned()
        else {
            return Err(ComptimeError {
                span,
                message: "comptime expression can only call `comptime fn`".to_string(),
            });
        };
        let type_substitutions = self.instantiate_resolved_function_generics(
            span,
            function_id.module_id,
            &signature,
            type_args,
            arg_exprs,
        )?;
        let return_ty = self
            .substitute_ty_into_current_module(
                function_id.module_id,
                signature.return_type,
                &type_substitutions,
            )
            .ok_or_else(|| ComptimeError {
                span,
                message: "cannot resolve comptime function return type".to_string(),
            })?;
        let Some(function) = self.comptime_function_body(function_id).cloned() else {
            return Err(ComptimeError {
                span,
                message: "comptime expression can only call `comptime fn`".to_string(),
            });
        };
        let value = nia_comptime_engine::eval_resolved_comptime_function_call(
            span,
            function_id.module_id,
            &function,
            type_substitutions.into_iter().collect(),
            args,
            self,
        )?;
        let return_ty = ComptimeValueType::Runtime(return_ty);
        let value = self.normalize_typed_comptime_value(value, &return_ty);
        self.validate_typed_value(span, &value, &return_ty);
        Ok(value)
    }

    fn bind_resolved_function_param(
        &mut self,
        span: Span,
        param: &ResolvedComptimeParam,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let ty = param
            .ty()
            .map(|ty| ComptimeValueType::Runtime(self.substitute_ty_generics(ty)));
        self.bind_local_value(span, param.local_id(), false, value, ty)
    }

    fn bind_resolved_function_local(
        &mut self,
        span: Span,
        binding: &ResolvedComptimeBinding,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let ty = binding
            .explicit_type()
            .map(|ty| ComptimeValueType::Runtime(self.substitute_ty_generics(ty)))
            .or_else(|| self.resolved_comptime_expr_type(binding.value(), None));
        self.bind_local_value(span, binding.local_id(), binding.is_mutable(), value, ty)
    }

    fn bind_resolved_pattern_local(
        &mut self,
        span: Span,
        _name: &str,
        local_id: LocalId,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let ty = self
            .find_local_binding_type(local_id)
            .map(|ty| ComptimeValueType::Runtime(self.substitute_ty_generics(ty)));
        self.bind_local_value(span, local_id, false, value, ty)
    }

    fn assign_resolved_local(
        &mut self,
        span: Span,
        target: &ResolvedComptimeAssignTarget,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        match target.kind() {
            ResolvedComptimeAssignTargetKind::Local { name, local_id, .. } => {
                self.assign_local_value(span, *local_id, name, value)
            }
        }
    }
}

impl Analyzer<'_> {
    fn bind_local_value(
        &mut self,
        span: Span,
        local_id: LocalId,
        is_mutable: bool,
        value: ComptimeValue,
        ty: Option<ComptimeValueType>,
    ) -> Result<(), ComptimeError> {
        let (value, ty) = if let Some(ty) = ty {
            let value = self.normalize_typed_comptime_value(value, &ty);
            self.validate_typed_value(span, &value, &ty);
            (value, Some(ty))
        } else {
            (value, None)
        };
        let Some(frame) = self.call_locals.last_mut() else {
            return Err(ComptimeError {
                span,
                message: "internal comptime function frame is missing".to_string(),
            });
        };
        if is_mutable {
            frame.mutable_locals.insert(local_id);
        }
        frame.locals.insert(local_id, value.clone());
        if let Some(ty) = ty {
            let typed = TypedComptimeValue { value, ty };
            frame.local_types.insert(local_id, typed.ty.clone());
        }
        Ok(())
    }

    fn assign_local_value(
        &mut self,
        span: Span,
        local_id: LocalId,
        name: &str,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        for index in (0..self.call_locals.len()).rev() {
            if !self.call_locals[index].locals.contains_key(&local_id) {
                continue;
            }
            if !self.call_locals[index].mutable_locals.contains(&local_id) {
                return Err(ComptimeError {
                    span,
                    message: format!("cannot assign to immutable comptime local `{name}`"),
                });
            }
            let previous_ty = self.call_locals[index].local_types.get(&local_id).cloned();
            let previous_value = self.call_locals[index].locals.get(&local_id).cloned();
            let value = if let Some(previous_ty) = previous_ty.as_ref() {
                let value = self.normalize_typed_comptime_value(value, previous_ty);
                self.validate_typed_value(span, &value, previous_ty);
                value
            } else {
                value
            };
            if let Some(previous_value) = previous_value.as_ref() {
                validate_assignment_shape(&mut self.diagnostics, span, &value, previous_value);
            }
            let frame = &mut self.call_locals[index];
            frame.locals.insert(local_id, value.clone());
            return Ok(());
        }
        Err(ComptimeError {
            span,
            message: format!("unknown comptime assignment target `{name}`"),
        })
    }
}

fn validate_assignment_shape(
    diagnostics: &mut Vec<Diagnostic>,
    span: Span,
    value: &ComptimeValue,
    previous: &ComptimeValue,
) {
    match (value, previous) {
        (ComptimeValue::Array(values), ComptimeValue::Array(previous_values)) => {
            if values.len() != previous_values.len() {
                diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "comptime array length {} does not match expected length {}",
                        values.len(),
                        previous_values.len()
                    ),
                ));
            }
            for (value, previous) in values.iter().zip(previous_values) {
                validate_assignment_shape(diagnostics, span, value, previous);
            }
        }
        (ComptimeValue::Struct(values), ComptimeValue::Struct(previous_values)) => {
            let previous_names = previous_values
                .keys()
                .map(String::as_str)
                .collect::<HashSet<_>>();
            for (name, previous) in previous_values {
                if let Some(value) = values.get(name) {
                    validate_assignment_shape(diagnostics, span, value, previous);
                } else {
                    diagnostics.push(Diagnostic::error(
                        span,
                        format!("comptime struct value is missing field `{name}`"),
                    ));
                }
            }
            for name in values.keys() {
                if !previous_names.contains(name.as_str()) {
                    diagnostics.push(Diagnostic::error(
                        span,
                        format!("comptime struct value has extra field `{name}`"),
                    ));
                }
            }
        }
        (ComptimeValue::Optional(Some(value)), ComptimeValue::Optional(Some(previous))) => {
            validate_assignment_shape(diagnostics, span, value, previous);
        }
        (ComptimeValue::ErrorUnion(Ok(value)), ComptimeValue::ErrorUnion(Ok(previous)))
        | (ComptimeValue::ErrorUnion(Err(value)), ComptimeValue::ErrorUnion(Err(previous))) => {
            validate_assignment_shape(diagnostics, span, value, previous);
        }
        _ => {}
    }
}

fn comptime_string_to_char_array(value: &str) -> Vec<ComptimeValue> {
    value
        .chars()
        .map(|value| ComptimeValue::Int(value as i128))
        .collect()
}

fn cast_comptime_integer(value: i128, ty: PrimitiveTy, pointer_width: u32) -> Option<i128> {
    let (bits, signed) = primitive_integer_layout(ty, pointer_width)?;
    let mask = integer_mask(bits)?;
    let raw = (value as u128) & mask;
    if signed {
        Some(sign_extend_integer(raw, bits))
    } else {
        i128::try_from(raw).ok()
    }
}

fn cast_int_to_float(value: i128, ty: PrimitiveTy) -> Option<f64> {
    let value = value as f64;
    cast_float_to_float(value, ty)
}

fn cast_float_to_float(value: f64, ty: PrimitiveTy) -> Option<f64> {
    let value = match ty {
        PrimitiveTy::F32 => f64::from(value as f32),
        PrimitiveTy::F64 => value,
        _ => value,
    };
    value.is_finite().then_some(value)
}

fn cast_float_to_integer(value: f64, ty: PrimitiveTy, pointer_width: u32) -> Option<i128> {
    if !value.is_finite() {
        return None;
    }
    let (min, max) = primitive_integer_range_for_target(ty, pointer_width)?;
    if value < min as f64 || value > max as f64 {
        return None;
    }
    Some(value.trunc() as i128)
}

fn primitive_integer_range_for_target(ty: PrimitiveTy, pointer_width: u32) -> Option<(i128, i128)> {
    match ty {
        PrimitiveTy::Isize => signed_integer_range(pointer_width),
        PrimitiveTy::Usize => unsigned_integer_range(pointer_width),
        _ => integer_range(ty),
    }
}

fn signed_integer_range(bits: u32) -> Option<(i128, i128)> {
    match bits {
        0 => None,
        1..=127 => Some((-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)),
        128 => Some((i128::MIN, i128::MAX)),
        _ => None,
    }
}

fn unsigned_integer_range(bits: u32) -> Option<(i128, i128)> {
    match bits {
        0 => None,
        1..=126 => Some((0, (1i128 << bits) - 1)),
        127 | 128 => Some((0, i128::MAX)),
        _ => None,
    }
}

fn primitive_integer_layout(ty: PrimitiveTy, pointer_width: u32) -> Option<(u32, bool)> {
    match ty {
        PrimitiveTy::I8 => Some((8, true)),
        PrimitiveTy::I16 => Some((16, true)),
        PrimitiveTy::I32 => Some((32, true)),
        PrimitiveTy::I64 => Some((64, true)),
        PrimitiveTy::I128 => Some((128, true)),
        PrimitiveTy::Isize => Some((pointer_width, true)),
        PrimitiveTy::U8 => Some((8, false)),
        PrimitiveTy::U16 => Some((16, false)),
        PrimitiveTy::U32 => Some((32, false)),
        PrimitiveTy::U64 => Some((64, false)),
        PrimitiveTy::U128 => Some((128, false)),
        PrimitiveTy::Usize => Some((pointer_width, false)),
        PrimitiveTy::Char => Some((32, false)),
        PrimitiveTy::Bool
        | PrimitiveTy::F32
        | PrimitiveTy::F64
        | PrimitiveTy::Void
        | PrimitiveTy::Never => None,
    }
}

fn is_float_primitive(ty: PrimitiveTy) -> bool {
    matches!(ty, PrimitiveTy::F32 | PrimitiveTy::F64)
}

fn integer_mask(bits: u32) -> Option<u128> {
    match bits {
        0 => None,
        1..=127 => Some((1u128 << bits) - 1),
        128 => Some(u128::MAX),
        _ => None,
    }
}

fn sign_extend_integer(raw: u128, bits: u32) -> i128 {
    if bits == 128 {
        return raw as i128;
    }
    let sign_bit = 1u128 << (bits - 1);
    if raw & sign_bit == 0 {
        raw as i128
    } else {
        (raw as i128) - (1i128 << bits)
    }
}

fn primitive_ty_name(primitive: PrimitiveTy) -> &'static str {
    match primitive {
        PrimitiveTy::I8 => "i8",
        PrimitiveTy::I16 => "i16",
        PrimitiveTy::I32 => "i32",
        PrimitiveTy::I64 => "i64",
        PrimitiveTy::I128 => "i128",
        PrimitiveTy::Isize => "isize",
        PrimitiveTy::U8 => "u8",
        PrimitiveTy::U16 => "u16",
        PrimitiveTy::U32 => "u32",
        PrimitiveTy::U64 => "u64",
        PrimitiveTy::U128 => "u128",
        PrimitiveTy::Usize => "usize",
        PrimitiveTy::F32 => "f32",
        PrimitiveTy::F64 => "f64",
        PrimitiveTy::Bool => "bool",
        PrimitiveTy::Char => "char",
        PrimitiveTy::Void => "void",
        PrimitiveTy::Never => "!",
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
        PrimitiveTy::Char => Some((0, 0x10FFFF)),
        PrimitiveTy::Bool
        | PrimitiveTy::F32
        | PrimitiveTy::F64
        | PrimitiveTy::Void
        | PrimitiveTy::Never => None,
    }
}

fn integer_literal_suffix_ty(text: &str) -> Option<PrimitiveTy> {
    Some(match numeric_literal_suffix(text)? {
        "i8" => PrimitiveTy::I8,
        "i16" => PrimitiveTy::I16,
        "i32" => PrimitiveTy::I32,
        "i64" => PrimitiveTy::I64,
        "i128" => PrimitiveTy::I128,
        "isize" => PrimitiveTy::Isize,
        "u8" => PrimitiveTy::U8,
        "u16" => PrimitiveTy::U16,
        "u32" => PrimitiveTy::U32,
        "u64" => PrimitiveTy::U64,
        "u128" => PrimitiveTy::U128,
        "usize" => PrimitiveTy::Usize,
        _ => return None,
    })
}

fn float_literal_suffix_ty(text: &str) -> Option<PrimitiveTy> {
    Some(match numeric_literal_suffix(text)? {
        "f32" => PrimitiveTy::F32,
        "f64" => PrimitiveTy::F64,
        _ => return None,
    })
}

fn numeric_literal_suffix(text: &str) -> Option<&str> {
    let non_decimal_radix = text.starts_with("0x")
        || text.starts_with("0X")
        || text.starts_with("0b")
        || text.starts_with("0B")
        || text.starts_with("0o")
        || text.starts_with("0O");
    let mut index = if non_decimal_radix { 2 } else { 0 };
    let bytes = text.as_bytes();
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'_'
            || if non_decimal_radix {
                byte.is_ascii_hexdigit()
            } else {
                byte.is_ascii_digit()
            }
        {
            index += 1;
        } else {
            break;
        }
    }
    (index < bytes.len()).then_some(&text[index..])
}

#[cfg(test)]
mod tests {
    use super::{
        ComptimeCheck, ComptimeInput, ComptimeKey, ComptimeModuleInput, ComptimeModuleLowering,
        ComptimeProgramContext, ComptimeValueType, check_module_comptime, lower_module_comptime,
    };
    use nia_comptime_ir::{EarlyComptimeExpr, EarlyComptimeExprKind, EarlyComptimeTypeArg};
    use nia_defs::{DefCollection, DefKind, ModuleId, collect_module_defs};
    use nia_item_signatures::collect_item_signatures;
    use nia_local_resolve::{LocalResolution, resolve_module_locals};
    use nia_parser::parse_module;
    use nia_span::Span;
    use nia_ty::{PrimitiveTy, TyKind};
    use nia_type_lower::{TypeLowering, lower_module_types_with_id};
    use nia_type_resolve::resolve_module_types;
    use nia_value_resolve::resolve_module_values;
    use std::collections::HashMap;

    struct CheckedFixture {
        defs: DefCollection,
        locals: LocalResolution,
        lowered: TypeLowering,
        comptime_module: ComptimeModuleLowering,
        checked: ComptimeCheck,
    }

    fn check_source(source: &str) -> CheckedFixture {
        let (module, errors) = parse_module(source);
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let type_names = resolve_module_types(&module, &defs);
        let lowered = lower_module_types_with_id(ModuleId(0), &module, &type_names);
        let signatures = collect_item_signatures(&module, &defs, &lowered);
        let values = resolve_module_values(&module, &defs);
        let locals = resolve_module_locals(&module, &defs, &values);
        let target = nia_target_config::TargetConfig::host();
        let comptime_module = lower_module_comptime(ComptimeModuleInput {
            module: &module,
            defs: &defs,
            values: &values,
            locals: &locals,
            type_uses: &lowered.type_uses,
            const_exprs: &lowered.const_exprs,
        });
        let checked = check_module_comptime(ComptimeInput {
            module: &comptime_module.module,
            defs: &defs,
            values: &values,
            locals: &locals,
            signatures: &signatures,
            interner: &lowered.interner,
            type_uses: &lowered.type_uses,
            normalized: &HashMap::new(),
            target: &target,
            program: ComptimeProgramContext::empty(),
        });
        CheckedFixture {
            defs,
            locals,
            lowered,
            comptime_module,
            checked,
        }
    }

    #[test]
    fn records_explicit_types_for_comptime_bindings() {
        let fixture = check_source(
            r#"
comptime let width: usize = 4;

fn main() i32 {
    comptime let local_width: usize = width;
    let xs: [local_width]i32 = [1, 2, 3, 4];
    xs[0]
}
"#,
        );
        assert!(
            fixture.comptime_module.diagnostics.is_empty(),
            "{:?}",
            fixture.comptime_module.diagnostics
        );
        assert!(
            fixture.checked.diagnostics.is_empty(),
            "{:?}",
            fixture.checked.diagnostics
        );
        let usize_ty = fixture.lowered.interner.primitive(PrimitiveTy::Usize);
        let width_def = fixture
            .defs
            .module_scope
            .values
            .get("width")
            .expect("width def");
        let width = fixture
            .checked
            .typed_values
            .get(&ComptimeKey::Global(super::GlobalDefId {
                module_id: ModuleId(0),
                def_id: width_def,
            }))
            .expect("typed global comptime value");
        assert_eq!(width.ty, ComptimeValueType::Runtime(usize_ty));
        assert!(fixture.locals.locals.iter().any(|(local_id, local)| {
            local.kind == nia_local_resolve::LocalKind::ComptimeBinding
                && fixture
                    .checked
                    .typed_values
                    .get(&ComptimeKey::Local(local_id))
                    .is_some_and(|typed| typed.ty == ComptimeValueType::Runtime(usize_ty))
        }));
    }

    #[test]
    fn records_enum_backing_types_for_comptime_variant_values() {
        let fixture = check_source(
            r#"
enum Code: u8 {
    ok = 1,
    fail = 2,
}
"#,
        );
        assert!(
            fixture.checked.diagnostics.is_empty(),
            "{:?}",
            fixture.checked.diagnostics
        );
        let u8_ty = fixture.lowered.interner.primitive(PrimitiveTy::U8);
        let variants = fixture
            .defs
            .defs
            .iter()
            .filter_map(|(def_id, def)| (def.kind == DefKind::EnumVariant).then_some(def_id));
        for variant in variants {
            let typed = fixture
                .checked
                .typed_enum_values
                .get(&variant)
                .expect("typed enum variant value");
            assert_eq!(typed.ty, ComptimeValueType::Runtime(u8_ty));
            assert!(matches!(
                typed
                    .ty
                    .runtime()
                    .and_then(|ty| fixture.lowered.interner.get(ty)),
                Some(TyKind::Primitive(PrimitiveTy::U8))
            ));
        }
    }

    #[test]
    fn semantic_comptime_lowering_requires_resolved_function_locals() {
        let (module, errors) = parse_module(
            r#"
comptime fn add_one(x: usize) usize {
    let y = x + 1;
    y
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let type_names = resolve_module_types(&module, &defs);
        let lowered = lower_module_types_with_id(ModuleId(0), &module, &type_names);
        let values = resolve_module_values(&module, &defs);
        let mut locals = resolve_module_locals(&module, &defs, &values);
        let removed = locals.local_defs.iter().find_map(|(span, local_id)| {
            let local = locals.locals.get(*local_id)?;
            (local.name == "y").then_some(*span)
        });
        let removed = removed.expect("local y span");
        locals.local_defs.remove(&removed);

        let comptime_module = lower_module_comptime(ComptimeModuleInput {
            module: &module,
            defs: &defs,
            values: &values,
            locals: &locals,
            type_uses: &lowered.type_uses,
            const_exprs: &lowered.const_exprs,
        });

        assert!(
            comptime_module.diagnostics.iter().any(|diagnostic| {
                diagnostic.span == removed
                    && diagnostic.message == "failed to resolve comptime local binding"
            }),
            "{:?}",
            comptime_module.diagnostics
        );
    }

    #[test]
    fn layout_builtin_requires_resolved_type_arg() {
        let expr = EarlyComptimeExpr {
            span: Span::new(0, 1),
            kind: EarlyComptimeExprKind::LayoutBuiltin {
                builtin: nia_ids::LayoutBuiltin::Size,
                type_arg: EarlyComptimeTypeArg {
                    span: Span::new(0, 1),
                    ty_span: Span::new(0, 1),
                    ty: None,
                },
            },
        };

        let err =
            nia_comptime_ir::resolve_expr(expr).expect_err("layout builtin should not resolve");
        assert_eq!(err.message, "failed to resolve comptime type argument");
    }
}
