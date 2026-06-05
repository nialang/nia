// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, LayoutBuiltin, LocalId, ValueBuiltin};
use nia_span::Span;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResolvedComptimeModule {
    pub enums: Vec<ResolvedComptimeEnum>,
    pub global_initializers: HashMap<GlobalDefId, ResolvedComptimeExpr>,
    pub local_initializers: HashMap<LocalId, ResolvedComptimeLocalInitializer>,
    pub functions: HashMap<GlobalDefId, ResolvedComptimeFunction>,
    pub const_exprs: HashMap<GlobalConstExprId, ResolvedComptimeExpr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeLocalInitializer {
    pub explicit_type: Option<InternedTyId>,
    pub value: ResolvedComptimeExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeEnum {
    pub def_id: GlobalDefId,
    pub span: Span,
    pub variants: Vec<ResolvedComptimeEnumVariant>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeEnumVariant {
    pub def_id: GlobalDefId,
    pub span: Span,
    pub value: Option<ResolvedComptimeExpr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeExpr {
    pub span: Span,
    pub kind: ResolvedComptimeExprKind,
}

impl ResolvedComptimeExpr {
    fn new(expr: ComptimeExpr) -> Result<Self, ComptimeLowerError> {
        resolve_expr(expr)
    }

    pub fn name_resolution(&self) -> Option<ComptimeNameResolution> {
        match self.kind {
            ResolvedComptimeExprKind::Name(resolution) => Some(resolution),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeFunction {
    span: Span,
    params: Vec<ResolvedComptimeParam>,
    body: ResolvedComptimeBlock,
}

impl ResolvedComptimeFunction {
    fn new(function: ComptimeFunction) -> Result<Self, ComptimeLowerError> {
        resolve_function(function)
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn params(&self) -> &[ResolvedComptimeParam] {
        &self.params
    }

    pub fn body(&self) -> &ResolvedComptimeBlock {
        &self.body
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeParam {
    pub span: Span,
    pub name: String,
    pub local_id: LocalId,
    pub ty: Option<InternedTyId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeBlock {
    pub span: Span,
    pub stmts: Vec<ResolvedComptimeStmt>,
    pub tail: Option<Box<ResolvedComptimeExpr>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeStmt {
    pub span: Span,
    pub kind: ResolvedComptimeStmtKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedComptimeStmtKind {
    Binding(ResolvedComptimeBinding),
    Expr(ResolvedComptimeExpr),
    Return(Option<ResolvedComptimeExpr>),
    Break,
    Continue,
    If {
        cond: ResolvedComptimeExpr,
        then_branch: ResolvedComptimeBlock,
        else_branch: Option<ResolvedComptimeBlock>,
    },
    ForIn(ResolvedComptimeForIn),
    While {
        cond: ResolvedComptimeExpr,
        body: ResolvedComptimeBlock,
    },
    Loop {
        body: ResolvedComptimeBlock,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeBinding {
    pub span: Span,
    pub name: String,
    pub local_id: LocalId,
    pub explicit_type: Option<InternedTyId>,
    pub is_mutable: bool,
    pub value: ResolvedComptimeExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeAssign {
    pub lhs: ResolvedComptimeAssignTarget,
    pub op: ComptimeAssignOp,
    pub rhs: ResolvedComptimeExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedComptimeAssignTarget {
    Local {
        span: Span,
        name: String,
        local_id: LocalId,
        path: Vec<ResolvedComptimeAssignPathElem>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedComptimeAssignPathElem {
    Field {
        span: Span,
        name: String,
    },
    Index {
        span: Span,
        index: ResolvedComptimeExpr,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeForIn {
    pub binding: ResolvedComptimeForBinding,
    pub iter: ResolvedComptimeExpr,
    pub body: ResolvedComptimeBlock,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeForBinding {
    pub span: Span,
    pub name: String,
    pub local_id: LocalId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeSwitch {
    pub span: Span,
    pub target: ResolvedComptimeExpr,
    pub arms: Vec<ResolvedComptimeSwitchArm>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeSwitchArm {
    pub span: Span,
    pub patterns: Vec<ResolvedComptimeSwitchPattern>,
    pub body: ResolvedComptimeSwitchArmBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedComptimeSwitchPattern {
    Default,
    OptionalSome {
        name: String,
        local_id: LocalId,
        span: Span,
    },
    OptionalNull {
        span: Span,
    },
    ErrorOk {
        name: String,
        local_id: LocalId,
        span: Span,
    },
    ErrorErr {
        name: String,
        local_id: LocalId,
        span: Span,
    },
    Expr(ResolvedComptimeExpr),
    Range {
        start: ResolvedComptimeExpr,
        end: ResolvedComptimeExpr,
        inclusive: bool,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedComptimeSwitchArmBody {
    Expr(ResolvedComptimeExpr),
    Stmt(ResolvedComptimeStmt),
    Block(ResolvedComptimeBlock),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedComptimeExprKind {
    Integer(String),
    Char(String),
    ByteChar(String),
    Float(String),
    String(ComptimeStringLiteral),
    ByteString(ComptimeStringLiteral),
    CString(ComptimeStringLiteral),
    Bool(bool),
    Null,
    Name(ComptimeNameResolution),
    Field {
        lhs: Box<ResolvedComptimeExpr>,
        name: String,
    },
    Len {
        lhs: Box<ResolvedComptimeExpr>,
    },
    RangeIter {
        lhs: Box<ResolvedComptimeExpr>,
    },
    Index {
        lhs: Box<ResolvedComptimeExpr>,
        index: Box<ResolvedComptimeExpr>,
    },
    Slice {
        lhs: Box<ResolvedComptimeExpr>,
        range: ResolvedComptimeSliceRange,
    },
    ArrayLiteral {
        ty: Option<InternedTyId>,
        elems: ResolvedComptimeArrayElements,
    },
    StructLiteral {
        ty: Option<InternedTyId>,
        fields: Vec<ResolvedComptimeFieldInit>,
    },
    BuiltinValue(ValueBuiltin),
    LayoutBuiltin {
        builtin: LayoutBuiltin,
        type_arg: ResolvedComptimeTypeArg,
    },
    Call {
        callee: Box<ResolvedComptimeExpr>,
        type_args: Vec<ResolvedComptimeTypeArg>,
        args: Vec<ResolvedComptimeExpr>,
    },
    Unary {
        op: ComptimeUnaryOp,
        expr: Box<ResolvedComptimeExpr>,
    },
    OptionalSome {
        expr: Box<ResolvedComptimeExpr>,
    },
    ErrorOk {
        expr: Box<ResolvedComptimeExpr>,
    },
    ErrorErr {
        expr: Box<ResolvedComptimeExpr>,
    },
    Try {
        expr: Box<ResolvedComptimeExpr>,
    },
    Binary {
        lhs: Box<ResolvedComptimeExpr>,
        op: ComptimeBinaryOp,
        rhs: Box<ResolvedComptimeExpr>,
    },
    Assign(Box<ResolvedComptimeAssign>),
    Range(ResolvedComptimeRange),
    If {
        cond: Box<ResolvedComptimeExpr>,
        then_branch: ResolvedComptimeBlock,
        else_branch: Option<Box<ResolvedComptimeExpr>>,
    },
    Switch(Box<ResolvedComptimeSwitch>),
    Cast {
        expr: Box<ResolvedComptimeExpr>,
        ty: InternedTyId,
    },
    Block(ResolvedComptimeBlock),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeRange {
    pub start: Option<Box<ResolvedComptimeExpr>>,
    pub end: Option<Box<ResolvedComptimeExpr>>,
    pub inclusive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeSliceRange {
    pub start: Option<Box<ResolvedComptimeExpr>>,
    pub end: Option<Box<ResolvedComptimeExpr>>,
    pub inclusive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedComptimeArrayElements {
    List(Vec<ResolvedComptimeExpr>),
    Repeat {
        value: Box<ResolvedComptimeExpr>,
        count: Box<ResolvedComptimeExpr>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeFieldInit {
    pub span: Span,
    pub name: String,
    pub value: ResolvedComptimeExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeTypeArg {
    pub span: Span,
    pub ty_span: Span,
    pub ty: InternedTyId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeFunction {
    pub span: Span,
    pub params: Vec<ComptimeParam>,
    pub body: ComptimeBlock,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeParam {
    pub span: Span,
    pub name: String,
    pub local_id: Option<LocalId>,
    pub ty: Option<InternedTyId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeBlock {
    pub span: Span,
    pub stmts: Vec<ComptimeStmt>,
    pub tail: Option<Box<ComptimeExpr>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeStmt {
    pub span: Span,
    pub kind: ComptimeStmtKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ComptimeStmtKind {
    Binding(ComptimeBinding),
    Expr(ComptimeExpr),
    Return(Option<ComptimeExpr>),
    Break,
    Continue,
    If {
        cond: ComptimeExpr,
        then_branch: ComptimeBlock,
        else_branch: Option<ComptimeBlock>,
    },
    ForIn(ComptimeForIn),
    While {
        cond: ComptimeExpr,
        body: ComptimeBlock,
    },
    Loop {
        body: ComptimeBlock,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeBinding {
    pub span: Span,
    pub name: String,
    pub local_id: Option<LocalId>,
    pub explicit_type: Option<InternedTyId>,
    pub is_mutable: bool,
    pub value: ComptimeExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeAssign {
    pub lhs: ComptimeAssignTarget,
    pub op: ComptimeAssignOp,
    pub rhs: ComptimeExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ComptimeAssignTarget {
    Local {
        span: Span,
        name: String,
        local_id: Option<LocalId>,
        path: Vec<ComptimeAssignPathElem>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ComptimeAssignPathElem {
    Field { span: Span, name: String },
    Index { span: Span, index: ComptimeExpr },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeForIn {
    pub binding: ComptimeForBinding,
    pub iter: ComptimeExpr,
    pub body: ComptimeBlock,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeForBinding {
    pub span: Span,
    pub name: String,
    pub local_id: Option<LocalId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeSwitch {
    pub span: Span,
    pub target: ComptimeExpr,
    pub arms: Vec<ComptimeSwitchArm>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeSwitchArm {
    pub span: Span,
    pub patterns: Vec<ComptimeSwitchPattern>,
    pub body: ComptimeSwitchArmBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ComptimeSwitchPattern {
    Default,
    OptionalSome {
        name: String,
        local_id: Option<LocalId>,
        span: Span,
    },
    OptionalNull {
        span: Span,
    },
    ErrorOk {
        name: String,
        local_id: Option<LocalId>,
        span: Span,
    },
    ErrorErr {
        name: String,
        local_id: Option<LocalId>,
        span: Span,
    },
    Expr(ComptimeExpr),
    Range {
        start: ComptimeExpr,
        end: ComptimeExpr,
        inclusive: bool,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ComptimeSwitchArmBody {
    Expr(ComptimeExpr),
    Stmt(ComptimeStmt),
    Block(ComptimeBlock),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeExpr {
    pub span: Span,
    pub kind: ComptimeExprKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ComptimeExprKind {
    Integer(String),
    Char(String),
    ByteChar(String),
    Float(String),
    String(ComptimeStringLiteral),
    ByteString(ComptimeStringLiteral),
    CString(ComptimeStringLiteral),
    Bool(bool),
    Null,
    Ident {
        name: String,
        resolution: Option<ComptimeNameResolution>,
    },
    Qualified {
        name: String,
        resolution: Option<ComptimeNameResolution>,
    },
    Field {
        lhs: Box<ComptimeExpr>,
        name: String,
    },
    Len {
        lhs: Box<ComptimeExpr>,
    },
    RangeIter {
        lhs: Box<ComptimeExpr>,
    },
    Index {
        lhs: Box<ComptimeExpr>,
        index: Box<ComptimeExpr>,
    },
    Slice {
        lhs: Box<ComptimeExpr>,
        range: ComptimeSliceRange,
    },
    ArrayLiteral {
        ty: Option<InternedTyId>,
        elems: ComptimeArrayElements,
    },
    StructLiteral {
        ty: Option<InternedTyId>,
        fields: Vec<ComptimeFieldInit>,
    },
    BuiltinValue(ValueBuiltin),
    LayoutBuiltin {
        builtin: LayoutBuiltin,
        type_arg: ComptimeTypeArg,
    },
    Call {
        callee: Box<ComptimeExpr>,
        type_args: Vec<ComptimeTypeArg>,
        args: Vec<ComptimeExpr>,
    },
    Unary {
        op: ComptimeUnaryOp,
        expr: Box<ComptimeExpr>,
    },
    OptionalSome {
        expr: Box<ComptimeExpr>,
    },
    ErrorOk {
        expr: Box<ComptimeExpr>,
    },
    ErrorErr {
        expr: Box<ComptimeExpr>,
    },
    Try {
        expr: Box<ComptimeExpr>,
    },
    Binary {
        lhs: Box<ComptimeExpr>,
        op: ComptimeBinaryOp,
        rhs: Box<ComptimeExpr>,
    },
    Assign(Box<ComptimeAssign>),
    Range(ComptimeRange),
    If {
        cond: Box<ComptimeExpr>,
        then_branch: ComptimeBlock,
        else_branch: Option<Box<ComptimeExpr>>,
    },
    Switch(Box<ComptimeSwitch>),
    Cast {
        expr: Box<ComptimeExpr>,
        ty: Option<InternedTyId>,
    },
    Block(ComptimeBlock),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComptimeStringLiteral {
    pub parts: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComptimeUnaryOp {
    Neg,
    Not,
    BitNot,
    RefReadOnly,
    Ref,
    Deref,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComptimeBinaryOp {
    Mul,
    Div,
    Rem,
    Add,
    Sub,
    Shl,
    Shr,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    BitAnd,
    BitXor,
    BitOr,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComptimeAssignOp {
    Assign,
    Add,
    Sub,
    Shl,
    Shr,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitXor,
    BitOr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeRange {
    pub start: Option<Box<ComptimeExpr>>,
    pub end: Option<Box<ComptimeExpr>>,
    pub inclusive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeSliceRange {
    pub start: Option<Box<ComptimeExpr>>,
    pub end: Option<Box<ComptimeExpr>>,
    pub inclusive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ComptimeArrayElements {
    List(Vec<ComptimeExpr>),
    Repeat {
        value: Box<ComptimeExpr>,
        count: Box<ComptimeExpr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComptimeNameResolution {
    Local(LocalId),
    Global(GlobalDefId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeFieldInit {
    pub span: Span,
    pub name: String,
    pub value: ComptimeExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeTypeArg {
    pub span: Span,
    pub ty_span: Span,
    pub ty: Option<InternedTyId>,
}

impl ComptimeTypeArg {
    fn from_type_ref(
        ty: &nia_ast::TypeRef,
        context: &dyn ComptimeLowerContext,
    ) -> Result<Self, ComptimeLowerError> {
        Ok(Self {
            span: ty.span,
            ty_span: ty.span,
            ty: lower_type_id(context, ty.span)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComptimeLowerError {
    pub span: Span,
    pub message: String,
}

pub fn lower_expr_early(expr: &nia_ast::Expr) -> Result<ComptimeExpr, ComptimeLowerError> {
    lower_expr_internal(expr, &EarlyComptimeLowerInputs::default())
}

pub fn lower_expr_early_with_context(
    expr: &nia_ast::Expr,
    context: &EarlyComptimeLowerInputs<'_>,
) -> Result<ComptimeExpr, ComptimeLowerError> {
    lower_expr_internal(expr, context)
}

#[derive(Clone, Copy, Default)]
pub struct EarlyComptimeLowerInputs<'a> {
    pub name_resolution: Option<&'a dyn Fn(Span) -> Option<ComptimeNameResolution>>,
    pub local_id: Option<&'a dyn Fn(Span) -> Option<LocalId>>,
    pub type_id: Option<&'a dyn Fn(Span) -> Option<InternedTyId>>,
}

impl<'a> EarlyComptimeLowerInputs<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_name_resolution(
        mut self,
        name_resolution: &'a dyn Fn(Span) -> Option<ComptimeNameResolution>,
    ) -> Self {
        self.name_resolution = Some(name_resolution);
        self
    }

    pub fn with_local_id(mut self, local_id: &'a dyn Fn(Span) -> Option<LocalId>) -> Self {
        self.local_id = Some(local_id);
        self
    }

    pub fn with_type_id(mut self, type_id: &'a dyn Fn(Span) -> Option<InternedTyId>) -> Self {
        self.type_id = Some(type_id);
        self
    }
}

#[derive(Clone, Copy)]
pub struct ResolvedComptimeLowerInputs<'a> {
    pub name_resolution: &'a dyn Fn(Span) -> Option<ComptimeNameResolution>,
    pub local_id: &'a dyn Fn(Span) -> Option<LocalId>,
    pub type_id: &'a dyn Fn(Span) -> Option<InternedTyId>,
}

impl<'a> ResolvedComptimeLowerInputs<'a> {
    pub fn new(
        name_resolution: &'a dyn Fn(Span) -> Option<ComptimeNameResolution>,
        local_id: &'a dyn Fn(Span) -> Option<LocalId>,
        type_id: &'a dyn Fn(Span) -> Option<InternedTyId>,
    ) -> Self {
        Self {
            name_resolution,
            local_id,
            type_id,
        }
    }
}

trait ComptimeLowerContext {
    fn resolve_name(
        &self,
        span: Span,
    ) -> Result<Option<ComptimeNameResolution>, ComptimeLowerError>;

    fn lower_local_id(&self, span: Span) -> Result<Option<LocalId>, ComptimeLowerError>;

    fn lower_type_id(&self, span: Span) -> Result<Option<InternedTyId>, ComptimeLowerError>;
}

impl ComptimeLowerContext for EarlyComptimeLowerInputs<'_> {
    fn resolve_name(
        &self,
        span: Span,
    ) -> Result<Option<ComptimeNameResolution>, ComptimeLowerError> {
        Ok(self.name_resolution.and_then(|resolve| resolve(span)))
    }

    fn lower_local_id(&self, span: Span) -> Result<Option<LocalId>, ComptimeLowerError> {
        Ok(self.local_id.and_then(|local_id| local_id(span)))
    }

    fn lower_type_id(&self, span: Span) -> Result<Option<InternedTyId>, ComptimeLowerError> {
        Ok(self.type_id.and_then(|type_id| type_id(span)))
    }
}

impl ComptimeLowerContext for ResolvedComptimeLowerInputs<'_> {
    fn resolve_name(
        &self,
        span: Span,
    ) -> Result<Option<ComptimeNameResolution>, ComptimeLowerError> {
        (self.name_resolution)(span)
            .map(Some)
            .ok_or_else(|| unresolved_error(span, "comptime name"))
    }

    fn lower_local_id(&self, span: Span) -> Result<Option<LocalId>, ComptimeLowerError> {
        (self.local_id)(span)
            .map(Some)
            .ok_or_else(|| unresolved_error(span, "comptime local binding"))
    }

    fn lower_type_id(&self, span: Span) -> Result<Option<InternedTyId>, ComptimeLowerError> {
        (self.type_id)(span)
            .map(Some)
            .ok_or_else(|| unresolved_error(span, "comptime type"))
    }
}

fn lower_expr_internal(
    expr: &nia_ast::Expr,
    context: &dyn ComptimeLowerContext,
) -> Result<ComptimeExpr, ComptimeLowerError> {
    let kind = match &expr.kind {
        nia_ast::ExprKind::Integer(text) => ComptimeExprKind::Integer(text.clone()),
        nia_ast::ExprKind::Char(text) => ComptimeExprKind::Char(text.clone()),
        nia_ast::ExprKind::ByteChar(text) => ComptimeExprKind::ByteChar(text.clone()),
        nia_ast::ExprKind::Float(text) => ComptimeExprKind::Float(text.clone()),
        nia_ast::ExprKind::String(literal) => {
            ComptimeExprKind::String(lower_string_literal(literal))
        }
        nia_ast::ExprKind::ByteString(literal) => {
            ComptimeExprKind::ByteString(lower_string_literal(literal))
        }
        nia_ast::ExprKind::CString(literal) => {
            ComptimeExprKind::CString(lower_string_literal(literal))
        }
        nia_ast::ExprKind::Bool(value) => ComptimeExprKind::Bool(*value),
        nia_ast::ExprKind::Null => ComptimeExprKind::Null,
        nia_ast::ExprKind::Ident(name) => ComptimeExprKind::Ident {
            name: name.clone(),
            resolution: resolve_name(context, expr.span)?,
        },
        nia_ast::ExprKind::Qualified { name, .. } => ComptimeExprKind::Qualified {
            name: name.clone(),
            resolution: resolve_name(context, expr.span)?,
        },
        nia_ast::ExprKind::Field { lhs, name } => ComptimeExprKind::Field {
            lhs: Box::new(lower_expr_internal(lhs, context)?),
            name: name.clone(),
        },
        nia_ast::ExprKind::BracketSuffix { callee, args } => {
            let [arg] = args.as_slice() else {
                return Err(ComptimeLowerError {
                    span: expr.span,
                    message: "comptime bracket suffix requires exactly one index argument"
                        .to_string(),
                });
            };
            let Some(index) = &arg.expr else {
                return Err(ComptimeLowerError {
                    span: arg.span,
                    message: "comptime bracket suffix requires an expression index".to_string(),
                });
            };
            ComptimeExprKind::Index {
                lhs: Box::new(lower_expr_internal(callee, context)?),
                index: Box::new(lower_expr_internal(index, context)?),
            }
        }
        nia_ast::ExprKind::Index { lhs, index } => match index {
            nia_ast::IndexArg::Expr(index) => ComptimeExprKind::Index {
                lhs: Box::new(lower_expr_internal(lhs, context)?),
                index: Box::new(lower_expr_internal(index, context)?),
            },
            nia_ast::IndexArg::Range(range) => ComptimeExprKind::Slice {
                lhs: Box::new(lower_expr_internal(lhs, context)?),
                range: lower_slice_range_with_context(range, context)?,
            },
        },
        nia_ast::ExprKind::ArrayLiteral { elems } => ComptimeExprKind::ArrayLiteral {
            ty: None,
            elems: lower_array_elements_with_context(elems, context)?,
        },
        nia_ast::ExprKind::TypedArrayLiteral { ty, elems } => ComptimeExprKind::ArrayLiteral {
            ty: lower_type_id(context, ty.span)?,
            elems: lower_array_elements_with_context(elems, context)?,
        },
        nia_ast::ExprKind::StructLiteral { fields } => ComptimeExprKind::StructLiteral {
            ty: None,
            fields: fields
                .iter()
                .map(|field| lower_field_init_with_context(field, context))
                .collect::<Result<Vec<_>, _>>()?,
        },
        nia_ast::ExprKind::TypedStructLiteral { ty, fields } => ComptimeExprKind::StructLiteral {
            ty: lower_type_id(context, ty.span)?,
            fields: fields
                .iter()
                .map(|field| lower_field_init_with_context(field, context))
                .collect::<Result<Vec<_>, _>>()?,
        },
        nia_ast::ExprKind::Builtin { name, type_arg } => {
            if let Some(type_arg) = type_arg {
                let Some(builtin) = LayoutBuiltin::from_name(name) else {
                    return Err(ComptimeLowerError {
                        span: expr.span,
                        message: format!("unsupported builtin in comptime expression: @{name}"),
                    });
                };
                ComptimeExprKind::LayoutBuiltin {
                    builtin,
                    type_arg: ComptimeTypeArg::from_type_ref(type_arg, context)?,
                }
            } else {
                let Some(builtin) = ValueBuiltin::from_name(name) else {
                    return Err(ComptimeLowerError {
                        span: expr.span,
                        message: format!(
                            "unsupported builtin value in comptime expression: @{name}"
                        ),
                    });
                };
                ComptimeExprKind::BuiltinValue(builtin)
            }
        }
        nia_ast::ExprKind::Call { callee, args } => lower_call_with_context(callee, args, context)?,
        nia_ast::ExprKind::Unary { op, expr } => ComptimeExprKind::Unary {
            op: lower_unary_op(*op),
            expr: Box::new(lower_expr_internal(expr, context)?),
        },
        nia_ast::ExprKind::OptionalSome { expr } => ComptimeExprKind::OptionalSome {
            expr: Box::new(lower_expr_internal(expr, context)?),
        },
        nia_ast::ExprKind::ErrorOk { expr } => ComptimeExprKind::ErrorOk {
            expr: Box::new(lower_expr_internal(expr, context)?),
        },
        nia_ast::ExprKind::ErrorErr { expr } => ComptimeExprKind::ErrorErr {
            expr: Box::new(lower_expr_internal(expr, context)?),
        },
        nia_ast::ExprKind::Try { expr } => ComptimeExprKind::Try {
            expr: Box::new(lower_expr_internal(expr, context)?),
        },
        nia_ast::ExprKind::Binary { lhs, op, rhs } => ComptimeExprKind::Binary {
            lhs: Box::new(lower_expr_internal(lhs, context)?),
            op: lower_binary_op(*op),
            rhs: Box::new(lower_expr_internal(rhs, context)?),
        },
        nia_ast::ExprKind::Assign { lhs, op, rhs } => {
            ComptimeExprKind::Assign(Box::new(ComptimeAssign {
                lhs: lower_assign_target_with_context(lhs, context)?,
                op: lower_assign_op(*op),
                rhs: lower_expr_internal(rhs, context)?,
            }))
        }
        nia_ast::ExprKind::Range(range) => {
            ComptimeExprKind::Range(lower_comptime_range_with_context(range, context)?)
        }
        nia_ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => ComptimeExprKind::If {
            cond: Box::new(lower_expr_internal(cond, context)?),
            then_branch: lower_block_with_context(then_branch, context)?,
            else_branch: else_branch
                .as_deref()
                .map(|else_branch| lower_expr_internal(else_branch, context))
                .transpose()?
                .map(Box::new),
        },
        nia_ast::ExprKind::ComptimeIf(comptime_if) => {
            lower_comptime_if_with_context(comptime_if, context)?
        }
        nia_ast::ExprKind::Switch(switch) => ComptimeExprKind::Switch(Box::new(
            lower_switch_with_context(expr.span, switch, context)?,
        )),
        nia_ast::ExprKind::Cast { expr, ty } => ComptimeExprKind::Cast {
            expr: Box::new(lower_expr_internal(expr, context)?),
            ty: lower_type_id(context, ty.span)?,
        },
        nia_ast::ExprKind::Block(block) => {
            ComptimeExprKind::Block(lower_block_with_context(block, context)?)
        }
        _ => {
            return Err(ComptimeLowerError {
                span: expr.span,
                message: "unsupported comptime expression".to_string(),
            });
        }
    };
    Ok(ComptimeExpr {
        span: expr.span,
        kind,
    })
}

pub fn lower_expr_resolved_with_context(
    expr: &nia_ast::Expr,
    context: &ResolvedComptimeLowerInputs<'_>,
) -> Result<ResolvedComptimeExpr, ComptimeLowerError> {
    let expr = lower_expr_internal(expr, context)?;
    ResolvedComptimeExpr::new(expr)
}

fn lower_string_literal(literal: &nia_ast::StringLiteral) -> ComptimeStringLiteral {
    ComptimeStringLiteral {
        parts: literal.parts.clone(),
    }
}

fn lower_unary_op(op: nia_ast::UnaryOp) -> ComptimeUnaryOp {
    match op {
        nia_ast::UnaryOp::Neg => ComptimeUnaryOp::Neg,
        nia_ast::UnaryOp::Not => ComptimeUnaryOp::Not,
        nia_ast::UnaryOp::BitNot => ComptimeUnaryOp::BitNot,
        nia_ast::UnaryOp::RefReadOnly => ComptimeUnaryOp::RefReadOnly,
        nia_ast::UnaryOp::Ref => ComptimeUnaryOp::Ref,
        nia_ast::UnaryOp::Deref => ComptimeUnaryOp::Deref,
    }
}

fn lower_binary_op(op: nia_ast::BinaryOp) -> ComptimeBinaryOp {
    match op {
        nia_ast::BinaryOp::Mul => ComptimeBinaryOp::Mul,
        nia_ast::BinaryOp::Div => ComptimeBinaryOp::Div,
        nia_ast::BinaryOp::Rem => ComptimeBinaryOp::Rem,
        nia_ast::BinaryOp::Add => ComptimeBinaryOp::Add,
        nia_ast::BinaryOp::Sub => ComptimeBinaryOp::Sub,
        nia_ast::BinaryOp::Shl => ComptimeBinaryOp::Shl,
        nia_ast::BinaryOp::Shr => ComptimeBinaryOp::Shr,
        nia_ast::BinaryOp::Lt => ComptimeBinaryOp::Lt,
        nia_ast::BinaryOp::Le => ComptimeBinaryOp::Le,
        nia_ast::BinaryOp::Gt => ComptimeBinaryOp::Gt,
        nia_ast::BinaryOp::Ge => ComptimeBinaryOp::Ge,
        nia_ast::BinaryOp::Eq => ComptimeBinaryOp::Eq,
        nia_ast::BinaryOp::Ne => ComptimeBinaryOp::Ne,
        nia_ast::BinaryOp::BitAnd => ComptimeBinaryOp::BitAnd,
        nia_ast::BinaryOp::BitXor => ComptimeBinaryOp::BitXor,
        nia_ast::BinaryOp::BitOr => ComptimeBinaryOp::BitOr,
        nia_ast::BinaryOp::And => ComptimeBinaryOp::And,
        nia_ast::BinaryOp::Or => ComptimeBinaryOp::Or,
    }
}

fn lower_assign_op(op: nia_ast::AssignOp) -> ComptimeAssignOp {
    match op {
        nia_ast::AssignOp::Assign => ComptimeAssignOp::Assign,
        nia_ast::AssignOp::Add => ComptimeAssignOp::Add,
        nia_ast::AssignOp::Sub => ComptimeAssignOp::Sub,
        nia_ast::AssignOp::Shl => ComptimeAssignOp::Shl,
        nia_ast::AssignOp::Shr => ComptimeAssignOp::Shr,
        nia_ast::AssignOp::Mul => ComptimeAssignOp::Mul,
        nia_ast::AssignOp::Div => ComptimeAssignOp::Div,
        nia_ast::AssignOp::Rem => ComptimeAssignOp::Rem,
        nia_ast::AssignOp::BitAnd => ComptimeAssignOp::BitAnd,
        nia_ast::AssignOp::BitXor => ComptimeAssignOp::BitXor,
        nia_ast::AssignOp::BitOr => ComptimeAssignOp::BitOr,
    }
}

fn lower_call_with_context(
    callee: &nia_ast::Expr,
    args: &[nia_ast::Expr],
    context: &dyn ComptimeLowerContext,
) -> Result<ComptimeExprKind, ComptimeLowerError> {
    if args.is_empty()
        && let nia_ast::ExprKind::Field { lhs, name } = &callee.kind
        && name == "len"
    {
        return Ok(ComptimeExprKind::Len {
            lhs: Box::new(lower_expr_internal(lhs, context)?),
        });
    }
    if args.is_empty()
        && let nia_ast::ExprKind::Field { lhs, name } = &callee.kind
        && name == "iter"
    {
        return Ok(ComptimeExprKind::RangeIter {
            lhs: Box::new(lower_expr_internal(lhs, context)?),
        });
    }
    let (callee, type_args) = match &callee.kind {
        nia_ast::ExprKind::BracketSuffix {
            callee: generic_callee,
            args: bracket_args,
        } if bracket_args.iter().all(|arg| arg.ty.is_some()) => (
            generic_callee.as_ref(),
            lower_type_args_with_context(bracket_args, context)?,
        ),
        _ => (callee, Vec::new()),
    };
    Ok(ComptimeExprKind::Call {
        callee: Box::new(lower_expr_internal(callee, context)?),
        type_args,
        args: args
            .iter()
            .map(|arg| lower_expr_internal(arg, context))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn lower_comptime_range_with_context(
    range: &nia_ast::SliceRange,
    context: &dyn ComptimeLowerContext,
) -> Result<ComptimeRange, ComptimeLowerError> {
    Ok(ComptimeRange {
        start: range
            .start
            .as_deref()
            .map(|start| lower_expr_internal(start, context))
            .transpose()?
            .map(Box::new),
        end: range
            .end
            .as_deref()
            .map(|end| lower_expr_internal(end, context))
            .transpose()?
            .map(Box::new),
        inclusive: range.inclusive,
    })
}

fn lower_slice_range_with_context(
    range: &nia_ast::SliceRange,
    context: &dyn ComptimeLowerContext,
) -> Result<ComptimeSliceRange, ComptimeLowerError> {
    let range = lower_comptime_range_with_context(range, context)?;
    Ok(ComptimeSliceRange {
        start: range.start,
        end: range.end,
        inclusive: range.inclusive,
    })
}

fn lower_comptime_if_with_context(
    comptime_if: &nia_ast::ComptimeIfExpr,
    context: &dyn ComptimeLowerContext,
) -> Result<ComptimeExprKind, ComptimeLowerError> {
    Ok(ComptimeExprKind::If {
        cond: Box::new(lower_expr_internal(&comptime_if.cond, context)?),
        then_branch: lower_block_with_context(&comptime_if.then_branch, context)?,
        else_branch: comptime_if
            .else_branch
            .as_deref()
            .map(|else_branch| lower_expr_internal(else_branch, context))
            .transpose()?
            .map(Box::new),
    })
}

fn lower_type_args_with_context(
    args: &[nia_ast::BracketArg],
    context: &dyn ComptimeLowerContext,
) -> Result<Vec<ComptimeTypeArg>, ComptimeLowerError> {
    args.iter()
        .map(|arg| {
            let Some(ty) = &arg.ty else {
                return Err(ComptimeLowerError {
                    span: arg.span,
                    message: "comptime generic function arguments must be types".to_string(),
                });
            };
            Ok(ComptimeTypeArg {
                span: arg.span,
                ty_span: ty.span,
                ty: lower_type_id(context, ty.span)?,
            })
        })
        .collect()
}

fn lower_assign_target_with_context(
    expr: &nia_ast::Expr,
    context: &dyn ComptimeLowerContext,
) -> Result<ComptimeAssignTarget, ComptimeLowerError> {
    let mut path = Vec::new();
    let (span, name, local_id) = lower_assign_target_base_with_context(expr, context, &mut path)?;
    Ok(ComptimeAssignTarget::Local {
        span,
        name,
        local_id,
        path,
    })
}

fn lower_assign_target_base_with_context(
    expr: &nia_ast::Expr,
    context: &dyn ComptimeLowerContext,
    path: &mut Vec<ComptimeAssignPathElem>,
) -> Result<(Span, String, Option<LocalId>), ComptimeLowerError> {
    match &expr.kind {
        nia_ast::ExprKind::Ident(name) => {
            Ok((expr.span, name.clone(), lower_local_id(context, expr.span)?))
        }
        nia_ast::ExprKind::Field { lhs, name } => {
            let base = lower_assign_target_base_with_context(lhs, context, path)?;
            path.push(ComptimeAssignPathElem::Field {
                span: expr.span,
                name: name.clone(),
            });
            Ok(base)
        }
        nia_ast::ExprKind::Index { lhs, index } => {
            let base = lower_assign_target_base_with_context(lhs, context, path)?;
            let nia_ast::IndexArg::Expr(index) = index else {
                return Err(ComptimeLowerError {
                    span: expr.span,
                    message: "comptime assignment target does not support slicing".to_string(),
                });
            };
            path.push(ComptimeAssignPathElem::Index {
                span: expr.span,
                index: lower_expr_internal(index, context)?,
            });
            Ok(base)
        }
        nia_ast::ExprKind::BracketSuffix { callee, args } => {
            let base = lower_assign_target_base_with_context(callee, context, path)?;
            let [arg] = args.as_slice() else {
                return Err(ComptimeLowerError {
                    span: expr.span,
                    message: "comptime assignment target bracket suffix requires exactly one index argument".to_string(),
                });
            };
            let Some(index) = &arg.expr else {
                return Err(ComptimeLowerError {
                    span: arg.span,
                    message:
                        "comptime assignment target bracket suffix requires an expression index"
                            .to_string(),
                });
            };
            path.push(ComptimeAssignPathElem::Index {
                span: expr.span,
                index: lower_expr_internal(index, context)?,
            });
            Ok(base)
        }
        _ => Err(ComptimeLowerError {
            span: expr.span,
            message: "unsupported comptime assignment target".to_string(),
        }),
    }
}

fn lower_array_elements_with_context(
    elems: &nia_ast::ArrayElements,
    context: &dyn ComptimeLowerContext,
) -> Result<ComptimeArrayElements, ComptimeLowerError> {
    match elems {
        nia_ast::ArrayElements::List(elems) => Ok(ComptimeArrayElements::List(
            elems
                .iter()
                .map(|elem| lower_expr_internal(elem, context))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        nia_ast::ArrayElements::Repeat { value, count } => Ok(ComptimeArrayElements::Repeat {
            value: Box::new(lower_expr_internal(value, context)?),
            count: Box::new(lower_expr_internal(count, context)?),
        }),
    }
}

fn resolve_name(
    context: &dyn ComptimeLowerContext,
    span: Span,
) -> Result<Option<ComptimeNameResolution>, ComptimeLowerError> {
    context.resolve_name(span)
}

fn lower_local_id(
    context: &dyn ComptimeLowerContext,
    span: Span,
) -> Result<Option<LocalId>, ComptimeLowerError> {
    context.lower_local_id(span)
}

fn lower_type_id(
    context: &dyn ComptimeLowerContext,
    span: Span,
) -> Result<Option<InternedTyId>, ComptimeLowerError> {
    context.lower_type_id(span)
}

pub fn resolve_function(
    function: ComptimeFunction,
) -> Result<ResolvedComptimeFunction, ComptimeLowerError> {
    let params = function
        .params
        .into_iter()
        .map(resolve_comptime_param)
        .collect::<Result<Vec<_>, _>>()?;
    let body = resolve_comptime_block(function.body)?;
    Ok(ResolvedComptimeFunction {
        span: function.span,
        params,
        body,
    })
}

fn resolve_comptime_param(
    param: ComptimeParam,
) -> Result<ResolvedComptimeParam, ComptimeLowerError> {
    let local_id = param
        .local_id
        .ok_or_else(|| unresolved_error(param.span, "comptime function parameter local"))?;
    Ok(ResolvedComptimeParam {
        span: param.span,
        name: param.name,
        local_id,
        ty: param.ty,
    })
}

fn resolve_comptime_block(
    block: ComptimeBlock,
) -> Result<ResolvedComptimeBlock, ComptimeLowerError> {
    let stmts = block
        .stmts
        .into_iter()
        .map(resolve_comptime_stmt)
        .collect::<Result<Vec<_>, _>>()?;
    let tail = block
        .tail
        .map(|tail| resolve_expr(*tail).map(Box::new))
        .transpose()?;
    Ok(ResolvedComptimeBlock {
        span: block.span,
        stmts,
        tail,
    })
}

fn resolve_comptime_stmt(stmt: ComptimeStmt) -> Result<ResolvedComptimeStmt, ComptimeLowerError> {
    let kind = match stmt.kind {
        ComptimeStmtKind::Binding(binding) => {
            ResolvedComptimeStmtKind::Binding(resolve_comptime_binding(binding)?)
        }
        ComptimeStmtKind::Expr(expr) => ResolvedComptimeStmtKind::Expr(resolve_expr(expr)?),
        ComptimeStmtKind::Return(expr) => {
            ResolvedComptimeStmtKind::Return(expr.map(resolve_expr).transpose()?)
        }
        ComptimeStmtKind::Break => ResolvedComptimeStmtKind::Break,
        ComptimeStmtKind::Continue => ResolvedComptimeStmtKind::Continue,
        ComptimeStmtKind::If {
            cond,
            then_branch,
            else_branch,
        } => ResolvedComptimeStmtKind::If {
            cond: resolve_expr(cond)?,
            then_branch: resolve_comptime_block(then_branch)?,
            else_branch: else_branch.map(resolve_comptime_block).transpose()?,
        },
        ComptimeStmtKind::ForIn(for_in) => {
            ResolvedComptimeStmtKind::ForIn(resolve_comptime_for_in(for_in)?)
        }
        ComptimeStmtKind::While { cond, body } => ResolvedComptimeStmtKind::While {
            cond: resolve_expr(cond)?,
            body: resolve_comptime_block(body)?,
        },
        ComptimeStmtKind::Loop { body } => ResolvedComptimeStmtKind::Loop {
            body: resolve_comptime_block(body)?,
        },
    };
    Ok(ResolvedComptimeStmt {
        span: stmt.span,
        kind,
    })
}

fn resolve_comptime_binding(
    binding: ComptimeBinding,
) -> Result<ResolvedComptimeBinding, ComptimeLowerError> {
    let local_id = binding
        .local_id
        .ok_or_else(|| unresolved_error(binding.span, "comptime local binding"))?;
    Ok(ResolvedComptimeBinding {
        span: binding.span,
        name: binding.name,
        local_id,
        explicit_type: binding.explicit_type,
        is_mutable: binding.is_mutable,
        value: resolve_expr(binding.value)?,
    })
}

fn resolve_comptime_for_in(
    for_in: ComptimeForIn,
) -> Result<ResolvedComptimeForIn, ComptimeLowerError> {
    let local_id = for_in
        .binding
        .local_id
        .ok_or_else(|| unresolved_error(for_in.binding.span, "comptime for binding"))?;
    Ok(ResolvedComptimeForIn {
        binding: ResolvedComptimeForBinding {
            span: for_in.binding.span,
            name: for_in.binding.name,
            local_id,
        },
        iter: resolve_expr(for_in.iter)?,
        body: resolve_comptime_block(for_in.body)?,
    })
}

pub fn resolve_expr(expr: ComptimeExpr) -> Result<ResolvedComptimeExpr, ComptimeLowerError> {
    let span = expr.span;
    let kind = match expr.kind {
        ComptimeExprKind::Integer(value) => ResolvedComptimeExprKind::Integer(value),
        ComptimeExprKind::Char(value) => ResolvedComptimeExprKind::Char(value),
        ComptimeExprKind::ByteChar(value) => ResolvedComptimeExprKind::ByteChar(value),
        ComptimeExprKind::Float(value) => ResolvedComptimeExprKind::Float(value),
        ComptimeExprKind::String(value) => ResolvedComptimeExprKind::String(value),
        ComptimeExprKind::ByteString(value) => ResolvedComptimeExprKind::ByteString(value),
        ComptimeExprKind::CString(value) => ResolvedComptimeExprKind::CString(value),
        ComptimeExprKind::Bool(value) => ResolvedComptimeExprKind::Bool(value),
        ComptimeExprKind::Null => ResolvedComptimeExprKind::Null,
        ComptimeExprKind::Ident { resolution, .. }
        | ComptimeExprKind::Qualified { resolution, .. } => ResolvedComptimeExprKind::Name(
            resolution.ok_or_else(|| unresolved_error(span, "comptime name"))?,
        ),
        ComptimeExprKind::Field { lhs, name } => ResolvedComptimeExprKind::Field {
            lhs: Box::new(resolve_expr(*lhs)?),
            name,
        },
        ComptimeExprKind::Len { lhs } => ResolvedComptimeExprKind::Len {
            lhs: Box::new(resolve_expr(*lhs)?),
        },
        ComptimeExprKind::RangeIter { lhs } => ResolvedComptimeExprKind::RangeIter {
            lhs: Box::new(resolve_expr(*lhs)?),
        },
        ComptimeExprKind::Index { lhs, index } => ResolvedComptimeExprKind::Index {
            lhs: Box::new(resolve_expr(*lhs)?),
            index: Box::new(resolve_expr(*index)?),
        },
        ComptimeExprKind::Slice { lhs, range } => ResolvedComptimeExprKind::Slice {
            lhs: Box::new(resolve_expr(*lhs)?),
            range: resolve_comptime_slice_range(range)?,
        },
        ComptimeExprKind::ArrayLiteral { ty, elems } => ResolvedComptimeExprKind::ArrayLiteral {
            ty,
            elems: resolve_comptime_array_elements(elems)?,
        },
        ComptimeExprKind::StructLiteral { ty, fields } => ResolvedComptimeExprKind::StructLiteral {
            ty,
            fields: fields
                .into_iter()
                .map(resolve_comptime_field_init)
                .collect::<Result<Vec<_>, _>>()?,
        },
        ComptimeExprKind::BuiltinValue(builtin) => ResolvedComptimeExprKind::BuiltinValue(builtin),
        ComptimeExprKind::LayoutBuiltin { builtin, type_arg } => {
            ResolvedComptimeExprKind::LayoutBuiltin {
                builtin,
                type_arg: resolve_type_arg(type_arg)?,
            }
        }
        ComptimeExprKind::Call {
            callee,
            type_args,
            args,
        } => ResolvedComptimeExprKind::Call {
            callee: Box::new(resolve_expr(*callee)?),
            type_args: type_args
                .into_iter()
                .map(resolve_type_arg)
                .collect::<Result<Vec<_>, _>>()?,
            args: args
                .into_iter()
                .map(resolve_expr)
                .collect::<Result<Vec<_>, _>>()?,
        },
        ComptimeExprKind::Unary { op, expr } => ResolvedComptimeExprKind::Unary {
            op,
            expr: Box::new(resolve_expr(*expr)?),
        },
        ComptimeExprKind::OptionalSome { expr } => ResolvedComptimeExprKind::OptionalSome {
            expr: Box::new(resolve_expr(*expr)?),
        },
        ComptimeExprKind::ErrorOk { expr } => ResolvedComptimeExprKind::ErrorOk {
            expr: Box::new(resolve_expr(*expr)?),
        },
        ComptimeExprKind::ErrorErr { expr } => ResolvedComptimeExprKind::ErrorErr {
            expr: Box::new(resolve_expr(*expr)?),
        },
        ComptimeExprKind::Try { expr } => ResolvedComptimeExprKind::Try {
            expr: Box::new(resolve_expr(*expr)?),
        },
        ComptimeExprKind::Binary { lhs, op, rhs } => ResolvedComptimeExprKind::Binary {
            lhs: Box::new(resolve_expr(*lhs)?),
            op,
            rhs: Box::new(resolve_expr(*rhs)?),
        },
        ComptimeExprKind::Assign(assign) => {
            ResolvedComptimeExprKind::Assign(Box::new(resolve_comptime_assign(*assign)?))
        }
        ComptimeExprKind::Range(range) => {
            ResolvedComptimeExprKind::Range(resolve_comptime_range(range)?)
        }
        ComptimeExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => ResolvedComptimeExprKind::If {
            cond: Box::new(resolve_expr(*cond)?),
            then_branch: resolve_comptime_block(then_branch)?,
            else_branch: else_branch
                .map(|else_branch| resolve_expr(*else_branch).map(Box::new))
                .transpose()?,
        },
        ComptimeExprKind::Switch(switch) => {
            ResolvedComptimeExprKind::Switch(Box::new(resolve_comptime_switch(*switch)?))
        }
        ComptimeExprKind::Cast { expr, ty } => ResolvedComptimeExprKind::Cast {
            expr: Box::new(resolve_expr(*expr)?),
            ty: ty.ok_or_else(|| unresolved_error(span, "comptime cast type"))?,
        },
        ComptimeExprKind::Block(block) => {
            ResolvedComptimeExprKind::Block(resolve_comptime_block(block)?)
        }
    };
    Ok(ResolvedComptimeExpr { span, kind })
}

fn resolve_comptime_assign(
    assign: ComptimeAssign,
) -> Result<ResolvedComptimeAssign, ComptimeLowerError> {
    Ok(ResolvedComptimeAssign {
        lhs: resolve_comptime_assign_target(assign.lhs)?,
        op: assign.op,
        rhs: resolve_expr(assign.rhs)?,
    })
}

fn resolve_comptime_assign_target(
    target: ComptimeAssignTarget,
) -> Result<ResolvedComptimeAssignTarget, ComptimeLowerError> {
    match target {
        ComptimeAssignTarget::Local {
            span,
            name,
            local_id,
            path,
        } => {
            let local_id =
                local_id.ok_or_else(|| unresolved_error(span, "comptime assignment target"))?;
            Ok(ResolvedComptimeAssignTarget::Local {
                span,
                name,
                local_id,
                path: path
                    .into_iter()
                    .map(resolve_comptime_assign_path_elem)
                    .collect::<Result<Vec<_>, _>>()?,
            })
        }
    }
}

fn resolve_comptime_assign_path_elem(
    elem: ComptimeAssignPathElem,
) -> Result<ResolvedComptimeAssignPathElem, ComptimeLowerError> {
    match elem {
        ComptimeAssignPathElem::Field { span, name } => {
            Ok(ResolvedComptimeAssignPathElem::Field { span, name })
        }
        ComptimeAssignPathElem::Index { span, index } => {
            Ok(ResolvedComptimeAssignPathElem::Index {
                span,
                index: resolve_expr(index)?,
            })
        }
    }
}

fn resolve_comptime_switch(
    switch: ComptimeSwitch,
) -> Result<ResolvedComptimeSwitch, ComptimeLowerError> {
    Ok(ResolvedComptimeSwitch {
        span: switch.span,
        target: resolve_expr(switch.target)?,
        arms: switch
            .arms
            .into_iter()
            .map(resolve_comptime_switch_arm)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn resolve_comptime_switch_arm(
    arm: ComptimeSwitchArm,
) -> Result<ResolvedComptimeSwitchArm, ComptimeLowerError> {
    Ok(ResolvedComptimeSwitchArm {
        span: arm.span,
        patterns: arm
            .patterns
            .into_iter()
            .map(resolve_comptime_switch_pattern)
            .collect::<Result<Vec<_>, _>>()?,
        body: resolve_comptime_switch_arm_body(arm.body)?,
    })
}

fn resolve_comptime_switch_pattern(
    pattern: ComptimeSwitchPattern,
) -> Result<ResolvedComptimeSwitchPattern, ComptimeLowerError> {
    match pattern {
        ComptimeSwitchPattern::Default => Ok(ResolvedComptimeSwitchPattern::Default),
        ComptimeSwitchPattern::OptionalSome {
            name,
            local_id,
            span,
        } => Ok(ResolvedComptimeSwitchPattern::OptionalSome {
            name,
            local_id: local_id
                .ok_or_else(|| unresolved_error(span, "comptime switch pattern local"))?,
            span,
        }),
        ComptimeSwitchPattern::OptionalNull { span } => {
            Ok(ResolvedComptimeSwitchPattern::OptionalNull { span })
        }
        ComptimeSwitchPattern::ErrorOk {
            name,
            local_id,
            span,
        } => Ok(ResolvedComptimeSwitchPattern::ErrorOk {
            name,
            local_id: local_id
                .ok_or_else(|| unresolved_error(span, "comptime switch pattern local"))?,
            span,
        }),
        ComptimeSwitchPattern::ErrorErr {
            name,
            local_id,
            span,
        } => Ok(ResolvedComptimeSwitchPattern::ErrorErr {
            name,
            local_id: local_id
                .ok_or_else(|| unresolved_error(span, "comptime switch pattern local"))?,
            span,
        }),
        ComptimeSwitchPattern::Expr(expr) => {
            resolve_expr(expr).map(ResolvedComptimeSwitchPattern::Expr)
        }
        ComptimeSwitchPattern::Range {
            start,
            end,
            inclusive,
            span,
        } => Ok(ResolvedComptimeSwitchPattern::Range {
            start: resolve_expr(start)?,
            end: resolve_expr(end)?,
            inclusive,
            span,
        }),
    }
}

fn resolve_comptime_switch_arm_body(
    body: ComptimeSwitchArmBody,
) -> Result<ResolvedComptimeSwitchArmBody, ComptimeLowerError> {
    match body {
        ComptimeSwitchArmBody::Expr(expr) => {
            resolve_expr(expr).map(ResolvedComptimeSwitchArmBody::Expr)
        }
        ComptimeSwitchArmBody::Stmt(stmt) => {
            resolve_comptime_stmt(stmt).map(ResolvedComptimeSwitchArmBody::Stmt)
        }
        ComptimeSwitchArmBody::Block(block) => {
            resolve_comptime_block(block).map(ResolvedComptimeSwitchArmBody::Block)
        }
    }
}

fn resolve_comptime_array_elements(
    elems: ComptimeArrayElements,
) -> Result<ResolvedComptimeArrayElements, ComptimeLowerError> {
    match elems {
        ComptimeArrayElements::List(elems) => elems
            .into_iter()
            .map(resolve_expr)
            .collect::<Result<Vec<_>, _>>()
            .map(ResolvedComptimeArrayElements::List),
        ComptimeArrayElements::Repeat { value, count } => {
            Ok(ResolvedComptimeArrayElements::Repeat {
                value: Box::new(resolve_expr(*value)?),
                count: Box::new(resolve_expr(*count)?),
            })
        }
    }
}

fn resolve_comptime_range(
    range: ComptimeRange,
) -> Result<ResolvedComptimeRange, ComptimeLowerError> {
    Ok(ResolvedComptimeRange {
        start: range
            .start
            .map(|start| resolve_expr(*start).map(Box::new))
            .transpose()?,
        end: range
            .end
            .map(|end| resolve_expr(*end).map(Box::new))
            .transpose()?,
        inclusive: range.inclusive,
    })
}

fn resolve_comptime_slice_range(
    range: ComptimeSliceRange,
) -> Result<ResolvedComptimeSliceRange, ComptimeLowerError> {
    Ok(ResolvedComptimeSliceRange {
        start: range
            .start
            .map(|start| resolve_expr(*start).map(Box::new))
            .transpose()?,
        end: range
            .end
            .map(|end| resolve_expr(*end).map(Box::new))
            .transpose()?,
        inclusive: range.inclusive,
    })
}

fn resolve_comptime_field_init(
    field: ComptimeFieldInit,
) -> Result<ResolvedComptimeFieldInit, ComptimeLowerError> {
    Ok(ResolvedComptimeFieldInit {
        span: field.span,
        name: field.name,
        value: resolve_expr(field.value)?,
    })
}

pub fn resolve_type_arg(
    type_arg: ComptimeTypeArg,
) -> Result<ResolvedComptimeTypeArg, ComptimeLowerError> {
    Ok(ResolvedComptimeTypeArg {
        span: type_arg.span,
        ty_span: type_arg.ty_span,
        ty: type_arg
            .ty
            .ok_or_else(|| unresolved_error(type_arg.ty_span, "comptime type argument"))?,
    })
}

fn unresolved_error(span: Span, what: &str) -> ComptimeLowerError {
    ComptimeLowerError {
        span,
        message: format!("failed to resolve {what}"),
    }
}

pub fn lower_function_early(
    function_span: Span,
    function: &nia_ast::FunctionItem,
) -> Result<ComptimeFunction, ComptimeLowerError> {
    lower_function_internal(
        function_span,
        function,
        &EarlyComptimeLowerInputs::default(),
    )
}

fn lower_function_internal(
    function_span: Span,
    function: &nia_ast::FunctionItem,
    context: &dyn ComptimeLowerContext,
) -> Result<ComptimeFunction, ComptimeLowerError> {
    if !function.is_comptime || function.is_extern {
        return Err(ComptimeLowerError {
            span: function_span,
            message: "comptime expression can only call `comptime fn`".to_string(),
        });
    }
    let Some(body) = &function.body else {
        return Err(ComptimeLowerError {
            span: function_span,
            message: "comptime function requires a body".to_string(),
        });
    };
    let params = function
        .params
        .iter()
        .map(|param| {
            let Some(name) = &param.name else {
                return Err(ComptimeLowerError {
                    span: param.span,
                    message: "comptime function parameter requires a name".to_string(),
                });
            };
            Ok(ComptimeParam {
                span: param.span,
                name: name.clone(),
                local_id: lower_local_id(context, param.span)?,
                ty: param
                    .ty
                    .as_ref()
                    .map(|ty| lower_type_id(context, ty.span))
                    .transpose()?
                    .flatten(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ComptimeFunction {
        span: function_span,
        params,
        body: lower_block_with_context(body, context)?,
    })
}

pub fn lower_function_resolved_with_context(
    function_span: Span,
    function: &nia_ast::FunctionItem,
    context: &ResolvedComptimeLowerInputs<'_>,
) -> Result<ResolvedComptimeFunction, ComptimeLowerError> {
    let function = lower_function_internal(function_span, function, context)?;
    ResolvedComptimeFunction::new(function)
}

fn lower_block_with_context(
    block: &nia_ast::Block,
    context: &dyn ComptimeLowerContext,
) -> Result<ComptimeBlock, ComptimeLowerError> {
    Ok(ComptimeBlock {
        span: block.span,
        stmts: block
            .stmts
            .iter()
            .map(|stmt| lower_stmt_with_context(stmt, context))
            .collect::<Result<Vec<_>, _>>()?,
        tail: block
            .tail
            .as_deref()
            .map(|tail| lower_expr_internal(tail, context))
            .transpose()?
            .map(Box::new),
    })
}

fn lower_stmt_with_context(
    stmt: &nia_ast::Stmt,
    context: &dyn ComptimeLowerContext,
) -> Result<ComptimeStmt, ComptimeLowerError> {
    let kind = match &stmt.kind {
        nia_ast::StmtKind::Binding(binding) => {
            let Some(value) = &binding.value else {
                return Err(ComptimeLowerError {
                    span: stmt.span,
                    message: "comptime function binding requires an initializer".to_string(),
                });
            };
            ComptimeStmtKind::Binding(ComptimeBinding {
                span: stmt.span,
                name: binding.name.clone(),
                local_id: lower_local_id(context, stmt.span)?,
                explicit_type: binding
                    .ty
                    .as_ref()
                    .map(|ty| lower_type_id(context, ty.span))
                    .transpose()?
                    .flatten(),
                is_mutable: !binding.is_let,
                value: lower_expr_internal(value, context)?,
            })
        }
        nia_ast::StmtKind::Expr(expr) => lower_expr_stmt_with_context(expr, context)?,
        nia_ast::StmtKind::Return(value) => ComptimeStmtKind::Return(
            value
                .as_ref()
                .map(|value| lower_expr_internal(value, context))
                .transpose()?,
        ),
        nia_ast::StmtKind::Break => ComptimeStmtKind::Break,
        nia_ast::StmtKind::Continue => ComptimeStmtKind::Continue,
        nia_ast::StmtKind::ForIn(for_in) => ComptimeStmtKind::ForIn(ComptimeForIn {
            binding: ComptimeForBinding {
                span: for_in.binding.span,
                name: for_in.binding.name.clone(),
                local_id: lower_local_id(context, for_in.binding.span)?,
            },
            iter: lower_expr_internal(&for_in.iter, context)?,
            body: lower_block_with_context(&for_in.body, context)?,
        }),
        nia_ast::StmtKind::While(while_stmt) => ComptimeStmtKind::While {
            cond: lower_expr_internal(&while_stmt.cond, context)?,
            body: lower_block_with_context(&while_stmt.body, context)?,
        },
        nia_ast::StmtKind::Loop(loop_stmt) => ComptimeStmtKind::Loop {
            body: lower_block_with_context(&loop_stmt.body, context)?,
        },
        _ => {
            return Err(ComptimeLowerError {
                span: stmt.span,
                message: "unsupported statement in comptime function body".to_string(),
            });
        }
    };
    Ok(ComptimeStmt {
        span: stmt.span,
        kind,
    })
}

fn lower_expr_stmt_with_context(
    expr: &nia_ast::Expr,
    context: &dyn ComptimeLowerContext,
) -> Result<ComptimeStmtKind, ComptimeLowerError> {
    match &expr.kind {
        nia_ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => Ok(ComptimeStmtKind::If {
            cond: lower_expr_internal(cond, context)?,
            then_branch: lower_block_with_context(then_branch, context)?,
            else_branch: else_branch
                .as_deref()
                .map(|else_branch| lower_if_stmt_else_branch_with_context(else_branch, context))
                .transpose()?,
        }),
        _ => Ok(ComptimeStmtKind::Expr(lower_expr_internal(expr, context)?)),
    }
}

fn lower_if_stmt_else_branch_with_context(
    expr: &nia_ast::Expr,
    context: &dyn ComptimeLowerContext,
) -> Result<ComptimeBlock, ComptimeLowerError> {
    match &expr.kind {
        nia_ast::ExprKind::Block(block) => lower_block_with_context(block, context),
        nia_ast::ExprKind::If { .. } => Ok(ComptimeBlock {
            span: expr.span,
            stmts: vec![ComptimeStmt {
                span: expr.span,
                kind: lower_expr_stmt_with_context(expr, context)?,
            }],
            tail: None,
        }),
        _ => Ok(ComptimeBlock {
            span: expr.span,
            stmts: Vec::new(),
            tail: Some(Box::new(lower_expr_internal(expr, context)?)),
        }),
    }
}

fn lower_switch_with_context(
    span: Span,
    switch: &nia_ast::SwitchStmt,
    context: &dyn ComptimeLowerContext,
) -> Result<ComptimeSwitch, ComptimeLowerError> {
    Ok(ComptimeSwitch {
        span,
        target: lower_expr_internal(&switch.target, context)?,
        arms: switch
            .arms
            .iter()
            .map(|arm| lower_switch_arm_with_context(arm, context))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn lower_switch_arm_with_context(
    arm: &nia_ast::SwitchArm,
    context: &dyn ComptimeLowerContext,
) -> Result<ComptimeSwitchArm, ComptimeLowerError> {
    Ok(ComptimeSwitchArm {
        span: arm.span,
        patterns: arm
            .patterns
            .iter()
            .map(|pattern| lower_switch_pattern_with_context(pattern, context))
            .collect::<Result<Vec<_>, _>>()?,
        body: lower_switch_arm_body_with_context(&arm.body, context)?,
    })
}

fn lower_switch_pattern_with_context(
    pattern: &nia_ast::SwitchPattern,
    context: &dyn ComptimeLowerContext,
) -> Result<ComptimeSwitchPattern, ComptimeLowerError> {
    match pattern {
        nia_ast::SwitchPattern::Default => Ok(ComptimeSwitchPattern::Default),
        nia_ast::SwitchPattern::OptionalSome { name, span } => {
            Ok(ComptimeSwitchPattern::OptionalSome {
                name: name.clone(),
                local_id: lower_local_id(context, *span)?,
                span: *span,
            })
        }
        nia_ast::SwitchPattern::OptionalNull { span } => {
            Ok(ComptimeSwitchPattern::OptionalNull { span: *span })
        }
        nia_ast::SwitchPattern::ErrorOk { name, span } => Ok(ComptimeSwitchPattern::ErrorOk {
            name: name.clone(),
            local_id: lower_local_id(context, *span)?,
            span: *span,
        }),
        nia_ast::SwitchPattern::ErrorErr { name, span } => Ok(ComptimeSwitchPattern::ErrorErr {
            name: name.clone(),
            local_id: lower_local_id(context, *span)?,
            span: *span,
        }),
        nia_ast::SwitchPattern::Expr(expr) => {
            lower_expr_internal(expr, context).map(ComptimeSwitchPattern::Expr)
        }
        nia_ast::SwitchPattern::Range {
            start,
            end,
            inclusive,
            span,
        } => Ok(ComptimeSwitchPattern::Range {
            start: lower_expr_internal(start, context)?,
            end: lower_expr_internal(end, context)?,
            inclusive: *inclusive,
            span: *span,
        }),
    }
}

fn lower_switch_arm_body_with_context(
    body: &nia_ast::SwitchArmBody,
    context: &dyn ComptimeLowerContext,
) -> Result<ComptimeSwitchArmBody, ComptimeLowerError> {
    match body {
        nia_ast::SwitchArmBody::Expr(expr) => {
            lower_expr_internal(expr, context).map(ComptimeSwitchArmBody::Expr)
        }
        nia_ast::SwitchArmBody::Stmt(stmt) => {
            lower_stmt_with_context(stmt, context).map(ComptimeSwitchArmBody::Stmt)
        }
        nia_ast::SwitchArmBody::Block(block) => {
            lower_block_with_context(block, context).map(ComptimeSwitchArmBody::Block)
        }
    }
}

fn lower_field_init_with_context(
    field: &nia_ast::FieldInit,
    context: &dyn ComptimeLowerContext,
) -> Result<ComptimeFieldInit, ComptimeLowerError> {
    Ok(ComptimeFieldInit {
        span: field.span,
        name: field.name.clone(),
        value: lower_expr_internal(&field.value, context)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> Span {
        Span::new(0, 1)
    }

    fn int_expr(value: &str) -> ComptimeExpr {
        ComptimeExpr {
            span: span(),
            kind: ComptimeExprKind::Integer(value.to_string()),
        }
    }

    fn ast_ident(name: &str) -> nia_ast::Expr {
        nia_ast::Expr {
            span: span(),
            kind: nia_ast::ExprKind::Ident(name.to_string()),
        }
    }

    fn missing_name(_: Span) -> Option<ComptimeNameResolution> {
        None
    }

    fn missing_local(_: Span) -> Option<LocalId> {
        None
    }

    fn missing_type(_: Span) -> Option<InternedTyId> {
        None
    }

    #[test]
    fn resolved_expr_rejects_unresolved_names() {
        let expr = ComptimeExpr {
            span: span(),
            kind: ComptimeExprKind::Ident {
                name: "x".to_string(),
                resolution: None,
            },
        };

        let err = ResolvedComptimeExpr::new(expr).expect_err("unresolved name must be rejected");
        assert_eq!(err.message, "failed to resolve comptime name");
    }

    #[test]
    fn resolved_expr_rejects_unresolved_assignment_targets() {
        let expr = ComptimeExpr {
            span: span(),
            kind: ComptimeExprKind::Assign(Box::new(ComptimeAssign {
                lhs: ComptimeAssignTarget::Local {
                    span: span(),
                    name: "x".to_string(),
                    local_id: None,
                    path: Vec::new(),
                },
                op: ComptimeAssignOp::Assign,
                rhs: int_expr("1"),
            })),
        };

        let err = ResolvedComptimeExpr::new(expr)
            .expect_err("unresolved assignment target must be rejected");
        assert_eq!(err.message, "failed to resolve comptime assignment target");
    }

    #[test]
    fn resolved_function_rejects_unresolved_locals() {
        let function = ComptimeFunction {
            span: span(),
            params: vec![ComptimeParam {
                span: span(),
                name: "x".to_string(),
                local_id: None,
                ty: None,
            }],
            body: ComptimeBlock {
                span: span(),
                stmts: Vec::new(),
                tail: None,
            },
        };

        let err = ResolvedComptimeFunction::new(function)
            .expect_err("unresolved function parameter must be rejected");
        assert_eq!(
            err.message,
            "failed to resolve comptime function parameter local"
        );
    }

    #[test]
    fn resolved_expr_rejects_unresolved_type_args() {
        let expr = ComptimeExpr {
            span: span(),
            kind: ComptimeExprKind::LayoutBuiltin {
                builtin: LayoutBuiltin::Size,
                type_arg: ComptimeTypeArg {
                    span: span(),
                    ty_span: span(),
                    ty: None,
                },
            },
        };

        let err =
            ResolvedComptimeExpr::new(expr).expect_err("unresolved type arg must be rejected");
        assert_eq!(err.message, "failed to resolve comptime type argument");
    }

    #[test]
    fn resolved_lowering_requires_name_resolution() {
        let context =
            ResolvedComptimeLowerInputs::new(&missing_name, &missing_local, &missing_type);
        let err = lower_expr_resolved_with_context(&ast_ident("x"), &context)
            .expect_err("resolved lowering must reject unresolved names");
        assert_eq!(err.message, "failed to resolve comptime name");
    }

    #[test]
    fn resolved_lowering_requires_local_ids() {
        let block = nia_ast::Block {
            span: span(),
            stmts: vec![nia_ast::Stmt {
                span: span(),
                kind: nia_ast::StmtKind::Binding(nia_ast::BindingStmt {
                    name: "x".to_string(),
                    ty: None,
                    value: Some(ast_ident("x")),
                    is_let: true,
                    is_comptime: true,
                }),
            }],
            tail: None,
        };
        let expr = nia_ast::Expr {
            span: span(),
            kind: nia_ast::ExprKind::Block(block),
        };
        let name_resolution = |_| Some(ComptimeNameResolution::Local(LocalId(0)));
        let context =
            ResolvedComptimeLowerInputs::new(&name_resolution, &missing_local, &missing_type);
        let err = lower_expr_resolved_with_context(&expr, &context)
            .expect_err("resolved lowering must reject unresolved local bindings");
        assert_eq!(err.message, "failed to resolve comptime local binding");
    }

    #[test]
    fn resolved_lowering_requires_type_ids() {
        let expr = nia_ast::Expr {
            span: span(),
            kind: nia_ast::ExprKind::Cast {
                expr: Box::new(nia_ast::Expr {
                    span: span(),
                    kind: nia_ast::ExprKind::Integer("1".to_string()),
                }),
                ty: nia_ast::TypeRef {
                    span: span(),
                    text: "i32".to_string(),
                    kind: nia_ast::TypeKind::Path {
                        segments: vec![nia_ast::TypePathSegment {
                            name: "i32".to_string(),
                            args: Vec::new(),
                        }],
                    },
                },
            },
        };
        let name_resolution = |_| Some(ComptimeNameResolution::Local(LocalId(0)));
        let local_id = |_| Some(LocalId(0));
        let context = ResolvedComptimeLowerInputs::new(&name_resolution, &local_id, &missing_type);
        let err = lower_expr_resolved_with_context(&expr, &context)
            .expect_err("resolved lowering must reject unresolved types");
        assert_eq!(err.message, "failed to resolve comptime type");
    }
}
