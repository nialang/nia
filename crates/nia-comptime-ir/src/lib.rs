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
    expr: ComptimeExpr,
}

impl ResolvedComptimeExpr {
    fn new(expr: ComptimeExpr) -> Result<Self, ComptimeLowerError> {
        validate_resolved_expr(&expr)?;
        Ok(Self { expr })
    }

    pub fn as_expr(&self) -> &ComptimeExpr {
        &self.expr
    }

    pub fn into_inner(self) -> ComptimeExpr {
        self.expr
    }
}

impl std::ops::Deref for ResolvedComptimeExpr {
    type Target = ComptimeExpr;

    fn deref(&self) -> &Self::Target {
        self.as_expr()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeFunction {
    function: ComptimeFunction,
}

impl ResolvedComptimeFunction {
    fn new(function: ComptimeFunction) -> Result<Self, ComptimeLowerError> {
        validate_resolved_function(&function)?;
        Ok(Self { function })
    }

    pub fn as_function(&self) -> &ComptimeFunction {
        &self.function
    }

    pub fn into_inner(self) -> ComptimeFunction {
        self.function
    }
}

impl std::ops::Deref for ResolvedComptimeFunction {
    type Target = ComptimeFunction;

    fn deref(&self) -> &Self::Target {
        self.as_function()
    }
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
        context: &ComptimeLowerInputs<'_>,
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
    lower_expr_internal(expr, &ComptimeLowerInputs::default())
}

pub fn lower_expr_early_with_context(
    expr: &nia_ast::Expr,
    context: &ComptimeLowerInputs<'_>,
) -> Result<ComptimeExpr, ComptimeLowerError> {
    lower_expr_internal(expr, context)
}

#[derive(Clone, Copy, Default)]
pub struct ComptimeLowerInputs<'a> {
    pub name_resolution: Option<&'a dyn Fn(Span) -> Option<ComptimeNameResolution>>,
    pub local_id: Option<&'a dyn Fn(Span) -> Option<LocalId>>,
    pub type_id: Option<&'a dyn Fn(Span) -> Option<InternedTyId>>,
    mode: ComptimeLowerMode,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum ComptimeLowerMode {
    #[default]
    Early,
    Resolved,
}

impl<'a> ComptimeLowerInputs<'a> {
    pub fn early() -> Self {
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

    fn with_mode(self, mode: ComptimeLowerMode) -> Self {
        Self { mode, ..self }
    }
}

fn lower_expr_internal(
    expr: &nia_ast::Expr,
    context: &ComptimeLowerInputs<'_>,
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
    context: &ComptimeLowerInputs<'_>,
) -> Result<ResolvedComptimeExpr, ComptimeLowerError> {
    let expr = lower_expr_internal(expr, &context.with_mode(ComptimeLowerMode::Resolved))?;
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
    context: &ComptimeLowerInputs<'_>,
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
    context: &ComptimeLowerInputs<'_>,
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
    context: &ComptimeLowerInputs<'_>,
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
    context: &ComptimeLowerInputs<'_>,
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
    context: &ComptimeLowerInputs<'_>,
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
    context: &ComptimeLowerInputs<'_>,
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
    context: &ComptimeLowerInputs<'_>,
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
    context: &ComptimeLowerInputs<'_>,
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
    context: &ComptimeLowerInputs<'_>,
    span: Span,
) -> Result<Option<ComptimeNameResolution>, ComptimeLowerError> {
    let resolution = context.name_resolution.and_then(|resolve| resolve(span));
    if context.mode == ComptimeLowerMode::Resolved && resolution.is_none() {
        return Err(ComptimeLowerError {
            span,
            message: "failed to resolve comptime name".to_string(),
        });
    }
    Ok(resolution)
}

fn lower_local_id(
    context: &ComptimeLowerInputs<'_>,
    span: Span,
) -> Result<Option<LocalId>, ComptimeLowerError> {
    let local_id = context.local_id.and_then(|local_id| local_id(span));
    if context.mode == ComptimeLowerMode::Resolved && local_id.is_none() {
        return Err(ComptimeLowerError {
            span,
            message: "failed to resolve comptime local binding".to_string(),
        });
    }
    Ok(local_id)
}

fn lower_type_id(
    context: &ComptimeLowerInputs<'_>,
    span: Span,
) -> Result<Option<InternedTyId>, ComptimeLowerError> {
    let ty = context.type_id.and_then(|type_id| type_id(span));
    if context.mode == ComptimeLowerMode::Resolved && ty.is_none() {
        return Err(ComptimeLowerError {
            span,
            message: "failed to resolve comptime type".to_string(),
        });
    }
    Ok(ty)
}

fn validate_resolved_function(function: &ComptimeFunction) -> Result<(), ComptimeLowerError> {
    for param in &function.params {
        if param.local_id.is_none() {
            return Err(unresolved_error(
                param.span,
                "comptime function parameter local",
            ));
        }
    }
    validate_resolved_block(&function.body)
}

fn validate_resolved_block(block: &ComptimeBlock) -> Result<(), ComptimeLowerError> {
    for stmt in &block.stmts {
        validate_resolved_stmt(stmt)?;
    }
    if let Some(tail) = &block.tail {
        validate_resolved_expr(tail)?;
    }
    Ok(())
}

fn validate_resolved_stmt(stmt: &ComptimeStmt) -> Result<(), ComptimeLowerError> {
    match &stmt.kind {
        ComptimeStmtKind::Binding(binding) => {
            if binding.local_id.is_none() {
                return Err(unresolved_error(binding.span, "comptime local binding"));
            }
            validate_resolved_expr(&binding.value)
        }
        ComptimeStmtKind::Expr(expr) => validate_resolved_expr(expr),
        ComptimeStmtKind::Return(expr) => {
            if let Some(expr) = expr {
                validate_resolved_expr(expr)?;
            }
            Ok(())
        }
        ComptimeStmtKind::Break | ComptimeStmtKind::Continue => Ok(()),
        ComptimeStmtKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            validate_resolved_expr(cond)?;
            validate_resolved_block(then_branch)?;
            if let Some(else_branch) = else_branch {
                validate_resolved_block(else_branch)?;
            }
            Ok(())
        }
        ComptimeStmtKind::ForIn(for_in) => {
            if for_in.binding.local_id.is_none() {
                return Err(unresolved_error(
                    for_in.binding.span,
                    "comptime for binding",
                ));
            }
            validate_resolved_expr(&for_in.iter)?;
            validate_resolved_block(&for_in.body)
        }
        ComptimeStmtKind::While { cond, body } => {
            validate_resolved_expr(cond)?;
            validate_resolved_block(body)
        }
        ComptimeStmtKind::Loop { body } => validate_resolved_block(body),
    }
}

fn validate_resolved_expr(expr: &ComptimeExpr) -> Result<(), ComptimeLowerError> {
    match &expr.kind {
        ComptimeExprKind::Integer(_)
        | ComptimeExprKind::Char(_)
        | ComptimeExprKind::ByteChar(_)
        | ComptimeExprKind::Float(_)
        | ComptimeExprKind::String(_)
        | ComptimeExprKind::ByteString(_)
        | ComptimeExprKind::CString(_)
        | ComptimeExprKind::Bool(_)
        | ComptimeExprKind::Null
        | ComptimeExprKind::BuiltinValue(_) => Ok(()),
        ComptimeExprKind::Ident { resolution, .. }
        | ComptimeExprKind::Qualified { resolution, .. } => resolution
            .is_some()
            .then_some(())
            .ok_or_else(|| unresolved_error(expr.span, "comptime name")),
        ComptimeExprKind::Field { lhs, .. }
        | ComptimeExprKind::Len { lhs }
        | ComptimeExprKind::RangeIter { lhs } => validate_resolved_expr(lhs),
        ComptimeExprKind::Index { lhs, index } => {
            validate_resolved_expr(lhs)?;
            validate_resolved_expr(index)
        }
        ComptimeExprKind::Slice { lhs, range } => {
            validate_resolved_expr(lhs)?;
            validate_resolved_slice_range(range)
        }
        ComptimeExprKind::ArrayLiteral { elems, .. } => validate_resolved_array_elements(elems),
        ComptimeExprKind::StructLiteral { fields, .. } => {
            for field in fields {
                validate_resolved_expr(&field.value)?;
            }
            Ok(())
        }
        ComptimeExprKind::LayoutBuiltin { type_arg, .. } => validate_resolved_type_arg(type_arg),
        ComptimeExprKind::Call {
            callee,
            type_args,
            args,
        } => {
            validate_resolved_expr(callee)?;
            for type_arg in type_args {
                validate_resolved_type_arg(type_arg)?;
            }
            for arg in args {
                validate_resolved_expr(arg)?;
            }
            Ok(())
        }
        ComptimeExprKind::Unary { expr, .. }
        | ComptimeExprKind::OptionalSome { expr }
        | ComptimeExprKind::ErrorOk { expr }
        | ComptimeExprKind::ErrorErr { expr }
        | ComptimeExprKind::Try { expr } => validate_resolved_expr(expr),
        ComptimeExprKind::Binary { lhs, rhs, .. } => {
            validate_resolved_expr(lhs)?;
            validate_resolved_expr(rhs)
        }
        ComptimeExprKind::Assign(assign) => {
            validate_resolved_assign_target(&assign.lhs)?;
            validate_resolved_expr(&assign.rhs)
        }
        ComptimeExprKind::Range(range) => validate_resolved_range(range),
        ComptimeExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            validate_resolved_expr(cond)?;
            validate_resolved_block(then_branch)?;
            if let Some(else_branch) = else_branch {
                validate_resolved_expr(else_branch)?;
            }
            Ok(())
        }
        ComptimeExprKind::Switch(switch) => validate_resolved_switch(switch),
        ComptimeExprKind::Cast { expr, ty } => {
            validate_resolved_expr(expr)?;
            ty.is_some()
                .then_some(())
                .ok_or_else(|| unresolved_error(expr.span, "comptime cast type"))
        }
        ComptimeExprKind::Block(block) => validate_resolved_block(block),
    }
}

fn validate_resolved_assign_target(
    target: &ComptimeAssignTarget,
) -> Result<(), ComptimeLowerError> {
    match target {
        ComptimeAssignTarget::Local {
            span,
            local_id,
            path,
            ..
        } => {
            if local_id.is_none() {
                return Err(unresolved_error(*span, "comptime assignment target"));
            }
            for elem in path {
                if let ComptimeAssignPathElem::Index { index, .. } = elem {
                    validate_resolved_expr(index)?;
                }
            }
            Ok(())
        }
    }
}

fn validate_resolved_switch(switch: &ComptimeSwitch) -> Result<(), ComptimeLowerError> {
    validate_resolved_expr(&switch.target)?;
    for arm in &switch.arms {
        for pattern in &arm.patterns {
            validate_resolved_switch_pattern(pattern)?;
        }
        validate_resolved_switch_arm_body(&arm.body)?;
    }
    Ok(())
}

fn validate_resolved_switch_pattern(
    pattern: &ComptimeSwitchPattern,
) -> Result<(), ComptimeLowerError> {
    match pattern {
        ComptimeSwitchPattern::Default | ComptimeSwitchPattern::OptionalNull { .. } => Ok(()),
        ComptimeSwitchPattern::OptionalSome { local_id, span, .. }
        | ComptimeSwitchPattern::ErrorOk { local_id, span, .. }
        | ComptimeSwitchPattern::ErrorErr { local_id, span, .. } => local_id
            .is_some()
            .then_some(())
            .ok_or_else(|| unresolved_error(*span, "comptime switch pattern local")),
        ComptimeSwitchPattern::Expr(expr) => validate_resolved_expr(expr),
        ComptimeSwitchPattern::Range { start, end, .. } => {
            validate_resolved_expr(start)?;
            validate_resolved_expr(end)
        }
    }
}

fn validate_resolved_switch_arm_body(
    body: &ComptimeSwitchArmBody,
) -> Result<(), ComptimeLowerError> {
    match body {
        ComptimeSwitchArmBody::Expr(expr) => validate_resolved_expr(expr),
        ComptimeSwitchArmBody::Stmt(stmt) => validate_resolved_stmt(stmt),
        ComptimeSwitchArmBody::Block(block) => validate_resolved_block(block),
    }
}

fn validate_resolved_array_elements(
    elems: &ComptimeArrayElements,
) -> Result<(), ComptimeLowerError> {
    match elems {
        ComptimeArrayElements::List(elems) => {
            for elem in elems {
                validate_resolved_expr(elem)?;
            }
            Ok(())
        }
        ComptimeArrayElements::Repeat { value, count } => {
            validate_resolved_expr(value)?;
            validate_resolved_expr(count)
        }
    }
}

fn validate_resolved_range(range: &ComptimeRange) -> Result<(), ComptimeLowerError> {
    if let Some(start) = &range.start {
        validate_resolved_expr(start)?;
    }
    if let Some(end) = &range.end {
        validate_resolved_expr(end)?;
    }
    Ok(())
}

fn validate_resolved_slice_range(range: &ComptimeSliceRange) -> Result<(), ComptimeLowerError> {
    if let Some(start) = &range.start {
        validate_resolved_expr(start)?;
    }
    if let Some(end) = &range.end {
        validate_resolved_expr(end)?;
    }
    Ok(())
}

fn validate_resolved_type_arg(type_arg: &ComptimeTypeArg) -> Result<(), ComptimeLowerError> {
    type_arg
        .ty
        .is_some()
        .then_some(())
        .ok_or_else(|| unresolved_error(type_arg.ty_span, "comptime type argument"))
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
    lower_function_internal(function_span, function, &ComptimeLowerInputs::default())
}

fn lower_function_internal(
    function_span: Span,
    function: &nia_ast::FunctionItem,
    context: &ComptimeLowerInputs<'_>,
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
    context: &ComptimeLowerInputs<'_>,
) -> Result<ResolvedComptimeFunction, ComptimeLowerError> {
    let function = lower_function_internal(
        function_span,
        function,
        &context.with_mode(ComptimeLowerMode::Resolved),
    )?;
    ResolvedComptimeFunction::new(function)
}

fn lower_block_with_context(
    block: &nia_ast::Block,
    context: &ComptimeLowerInputs<'_>,
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
    context: &ComptimeLowerInputs<'_>,
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
    context: &ComptimeLowerInputs<'_>,
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
    context: &ComptimeLowerInputs<'_>,
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
    context: &ComptimeLowerInputs<'_>,
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
    context: &ComptimeLowerInputs<'_>,
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
    context: &ComptimeLowerInputs<'_>,
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
    context: &ComptimeLowerInputs<'_>,
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
    context: &ComptimeLowerInputs<'_>,
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
}
