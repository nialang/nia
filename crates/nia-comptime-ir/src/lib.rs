// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{AssignOp, BinaryOp, StringLiteral, UnaryOp};
use nia_ids::{GlobalConstExprId, GlobalDefId, LocalId};
use nia_span::Span;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ComptimeModule {
    pub enums: Vec<ComptimeEnum>,
    pub global_initializers: HashMap<GlobalDefId, ComptimeExpr>,
    pub local_initializers: HashMap<LocalId, ComptimeExpr>,
    pub functions: HashMap<GlobalDefId, ComptimeFunction>,
    pub const_exprs: HashMap<GlobalConstExprId, ComptimeExpr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeEnum {
    pub def_id: GlobalDefId,
    pub span: Span,
    pub variants: Vec<ComptimeEnumVariant>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeEnumVariant {
    pub def_id: GlobalDefId,
    pub span: Span,
    pub value: Option<ComptimeExpr>,
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
    pub is_mutable: bool,
    pub value: ComptimeExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeAssign {
    pub lhs: ComptimeAssignTarget,
    pub op: AssignOp,
    pub rhs: ComptimeExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ComptimeAssignTarget {
    Local {
        span: Span,
        name: String,
        local_id: Option<LocalId>,
    },
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
    String(StringLiteral),
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
    Index {
        lhs: Box<ComptimeExpr>,
        index: Box<ComptimeExpr>,
    },
    ArrayLiteral {
        elems: ComptimeArrayElements,
    },
    StructLiteral {
        fields: Vec<ComptimeFieldInit>,
    },
    Builtin {
        name: String,
        type_arg_span: Option<Span>,
    },
    Call {
        callee: Box<ComptimeExpr>,
        args: Vec<ComptimeExpr>,
    },
    Unary {
        op: UnaryOp,
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
        op: BinaryOp,
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
    },
    Block(ComptimeBlock),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeRange {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComptimeLowerError {
    pub span: Span,
    pub message: String,
}

pub fn lower_expr(expr: &nia_ast::Expr) -> Result<ComptimeExpr, ComptimeLowerError> {
    lower_expr_with_context(expr, &ComptimeLowerContext::default())
}

#[derive(Default)]
pub struct ComptimeLowerContext<'a> {
    pub name_resolution: Option<&'a dyn Fn(Span) -> Option<ComptimeNameResolution>>,
    pub local_id: Option<&'a dyn Fn(Span) -> Option<LocalId>>,
}

pub fn lower_expr_with_context(
    expr: &nia_ast::Expr,
    context: &ComptimeLowerContext<'_>,
) -> Result<ComptimeExpr, ComptimeLowerError> {
    let kind = match &expr.kind {
        nia_ast::ExprKind::Integer(text) => ComptimeExprKind::Integer(text.clone()),
        nia_ast::ExprKind::String(literal) => ComptimeExprKind::String(literal.clone()),
        nia_ast::ExprKind::Bool(value) => ComptimeExprKind::Bool(*value),
        nia_ast::ExprKind::Null => ComptimeExprKind::Null,
        nia_ast::ExprKind::Ident(name) => ComptimeExprKind::Ident {
            name: name.clone(),
            resolution: resolve_name(context, expr.span),
        },
        nia_ast::ExprKind::Qualified { name, .. } => ComptimeExprKind::Qualified {
            name: name.clone(),
            resolution: resolve_name(context, expr.span),
        },
        nia_ast::ExprKind::Field { lhs, name } => ComptimeExprKind::Field {
            lhs: Box::new(lower_expr_with_context(lhs, context)?),
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
                lhs: Box::new(lower_expr_with_context(callee, context)?),
                index: Box::new(lower_expr_with_context(index, context)?),
            }
        }
        nia_ast::ExprKind::Index { lhs, index } => match index {
            nia_ast::IndexArg::Expr(index) => ComptimeExprKind::Index {
                lhs: Box::new(lower_expr_with_context(lhs, context)?),
                index: Box::new(lower_expr_with_context(index, context)?),
            },
            nia_ast::IndexArg::Range(_) => {
                return Err(ComptimeLowerError {
                    span: expr.span,
                    message: "comptime array slicing is not supported".to_string(),
                });
            }
        },
        nia_ast::ExprKind::ArrayLiteral { elems }
        | nia_ast::ExprKind::TypedArrayLiteral { elems, .. } => ComptimeExprKind::ArrayLiteral {
            elems: lower_array_elements_with_context(elems, context)?,
        },
        nia_ast::ExprKind::StructLiteral { fields }
        | nia_ast::ExprKind::TypedStructLiteral { fields, .. } => ComptimeExprKind::StructLiteral {
            fields: fields
                .iter()
                .map(|field| lower_field_init_with_context(field, context))
                .collect::<Result<Vec<_>, _>>()?,
        },
        nia_ast::ExprKind::Builtin { name, type_arg } => ComptimeExprKind::Builtin {
            name: name.clone(),
            type_arg_span: type_arg.as_ref().map(|ty| ty.span),
        },
        nia_ast::ExprKind::Call { callee, args } => ComptimeExprKind::Call {
            callee: Box::new(lower_expr_with_context(callee, context)?),
            args: args
                .iter()
                .map(|arg| lower_expr_with_context(arg, context))
                .collect::<Result<Vec<_>, _>>()?,
        },
        nia_ast::ExprKind::Unary { op, expr } => ComptimeExprKind::Unary {
            op: *op,
            expr: Box::new(lower_expr_with_context(expr, context)?),
        },
        nia_ast::ExprKind::OptionalSome { expr } => ComptimeExprKind::OptionalSome {
            expr: Box::new(lower_expr_with_context(expr, context)?),
        },
        nia_ast::ExprKind::ErrorOk { expr } => ComptimeExprKind::ErrorOk {
            expr: Box::new(lower_expr_with_context(expr, context)?),
        },
        nia_ast::ExprKind::ErrorErr { expr } => ComptimeExprKind::ErrorErr {
            expr: Box::new(lower_expr_with_context(expr, context)?),
        },
        nia_ast::ExprKind::Try { expr } => ComptimeExprKind::Try {
            expr: Box::new(lower_expr_with_context(expr, context)?),
        },
        nia_ast::ExprKind::Binary { lhs, op, rhs } => ComptimeExprKind::Binary {
            lhs: Box::new(lower_expr_with_context(lhs, context)?),
            op: *op,
            rhs: Box::new(lower_expr_with_context(rhs, context)?),
        },
        nia_ast::ExprKind::Assign { lhs, op, rhs } => {
            ComptimeExprKind::Assign(Box::new(ComptimeAssign {
                lhs: lower_assign_target_with_context(lhs, context)?,
                op: *op,
                rhs: lower_expr_with_context(rhs, context)?,
            }))
        }
        nia_ast::ExprKind::Range(range) => ComptimeExprKind::Range(ComptimeRange {
            start: range
                .start
                .as_deref()
                .map(|start| lower_expr_with_context(start, context))
                .transpose()?
                .map(Box::new),
            end: range
                .end
                .as_deref()
                .map(|end| lower_expr_with_context(end, context))
                .transpose()?
                .map(Box::new),
            inclusive: range.inclusive,
        }),
        nia_ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => ComptimeExprKind::If {
            cond: Box::new(lower_expr_with_context(cond, context)?),
            then_branch: lower_block_with_context(then_branch, context)?,
            else_branch: else_branch
                .as_deref()
                .map(|else_branch| lower_expr_with_context(else_branch, context))
                .transpose()?
                .map(Box::new),
        },
        nia_ast::ExprKind::Switch(switch) => ComptimeExprKind::Switch(Box::new(
            lower_switch_with_context(expr.span, switch, context)?,
        )),
        nia_ast::ExprKind::Cast { expr, .. } => ComptimeExprKind::Cast {
            expr: Box::new(lower_expr_with_context(expr, context)?),
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

fn lower_assign_target_with_context(
    expr: &nia_ast::Expr,
    context: &ComptimeLowerContext<'_>,
) -> Result<ComptimeAssignTarget, ComptimeLowerError> {
    match &expr.kind {
        nia_ast::ExprKind::Ident(name) => Ok(ComptimeAssignTarget::Local {
            span: expr.span,
            name: name.clone(),
            local_id: context.local_id.and_then(|local_id| local_id(expr.span)),
        }),
        _ => Err(ComptimeLowerError {
            span: expr.span,
            message: "unsupported comptime assignment target".to_string(),
        }),
    }
}

fn lower_array_elements_with_context(
    elems: &nia_ast::ArrayElements,
    context: &ComptimeLowerContext<'_>,
) -> Result<ComptimeArrayElements, ComptimeLowerError> {
    match elems {
        nia_ast::ArrayElements::List(elems) => Ok(ComptimeArrayElements::List(
            elems
                .iter()
                .map(|elem| lower_expr_with_context(elem, context))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        nia_ast::ArrayElements::Repeat { value, count } => Ok(ComptimeArrayElements::Repeat {
            value: Box::new(lower_expr_with_context(value, context)?),
            count: Box::new(lower_expr_with_context(count, context)?),
        }),
    }
}

fn resolve_name(context: &ComptimeLowerContext<'_>, span: Span) -> Option<ComptimeNameResolution> {
    context.name_resolution.and_then(|resolve| resolve(span))
}

pub fn lower_function(
    function_span: Span,
    function: &nia_ast::FunctionItem,
) -> Result<ComptimeFunction, ComptimeLowerError> {
    lower_function_with_context(function_span, function, &ComptimeLowerContext::default())
}

pub fn lower_function_with_locals(
    function_span: Span,
    function: &nia_ast::FunctionItem,
    local_id_for_span: &impl Fn(Span) -> Option<LocalId>,
) -> Result<ComptimeFunction, ComptimeLowerError> {
    let context = ComptimeLowerContext {
        name_resolution: None,
        local_id: Some(local_id_for_span),
    };
    lower_function_with_context(function_span, function, &context)
}

pub fn lower_function_with_context(
    function_span: Span,
    function: &nia_ast::FunctionItem,
    context: &ComptimeLowerContext<'_>,
) -> Result<ComptimeFunction, ComptimeLowerError> {
    if !function.is_comptime || function.is_extern {
        return Err(ComptimeLowerError {
            span: function_span,
            message: "comptime expression can only call `comptime fn`".to_string(),
        });
    }
    if !function.generics.is_empty() {
        return Err(ComptimeLowerError {
            span: function_span,
            message: "generic comptime functions are not supported yet".to_string(),
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
                local_id: context.local_id.and_then(|local_id| local_id(param.span)),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ComptimeFunction {
        span: function_span,
        params,
        body: lower_block_with_context(body, context)?,
    })
}

fn lower_block_with_context(
    block: &nia_ast::Block,
    context: &ComptimeLowerContext<'_>,
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
            .map(|tail| lower_expr_with_context(tail, context))
            .transpose()?
            .map(Box::new),
    })
}

fn lower_stmt_with_context(
    stmt: &nia_ast::Stmt,
    context: &ComptimeLowerContext<'_>,
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
                local_id: context.local_id.and_then(|local_id| local_id(stmt.span)),
                is_mutable: !binding.is_let,
                value: lower_expr_with_context(value, context)?,
            })
        }
        nia_ast::StmtKind::Expr(expr) => lower_expr_stmt_with_context(expr, context)?,
        nia_ast::StmtKind::Return(value) => ComptimeStmtKind::Return(
            value
                .as_ref()
                .map(|value| lower_expr_with_context(value, context))
                .transpose()?,
        ),
        nia_ast::StmtKind::Break => ComptimeStmtKind::Break,
        nia_ast::StmtKind::Continue => ComptimeStmtKind::Continue,
        nia_ast::StmtKind::ForIn(for_in) => ComptimeStmtKind::ForIn(ComptimeForIn {
            binding: ComptimeForBinding {
                span: for_in.binding.span,
                name: for_in.binding.name.clone(),
                local_id: context
                    .local_id
                    .and_then(|local_id| local_id(for_in.binding.span)),
            },
            iter: lower_expr_with_context(&for_in.iter, context)?,
            body: lower_block_with_context(&for_in.body, context)?,
        }),
        nia_ast::StmtKind::While(while_stmt) => ComptimeStmtKind::While {
            cond: lower_expr_with_context(&while_stmt.cond, context)?,
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
    context: &ComptimeLowerContext<'_>,
) -> Result<ComptimeStmtKind, ComptimeLowerError> {
    match &expr.kind {
        nia_ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => Ok(ComptimeStmtKind::If {
            cond: lower_expr_with_context(cond, context)?,
            then_branch: lower_block_with_context(then_branch, context)?,
            else_branch: else_branch
                .as_deref()
                .map(|else_branch| lower_if_stmt_else_branch_with_context(else_branch, context))
                .transpose()?,
        }),
        _ => Ok(ComptimeStmtKind::Expr(lower_expr_with_context(
            expr, context,
        )?)),
    }
}

fn lower_if_stmt_else_branch_with_context(
    expr: &nia_ast::Expr,
    context: &ComptimeLowerContext<'_>,
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
            tail: Some(Box::new(lower_expr_with_context(expr, context)?)),
        }),
    }
}

fn lower_switch_with_context(
    span: Span,
    switch: &nia_ast::SwitchStmt,
    context: &ComptimeLowerContext<'_>,
) -> Result<ComptimeSwitch, ComptimeLowerError> {
    Ok(ComptimeSwitch {
        span,
        target: lower_expr_with_context(&switch.target, context)?,
        arms: switch
            .arms
            .iter()
            .map(|arm| lower_switch_arm_with_context(arm, context))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn lower_switch_arm_with_context(
    arm: &nia_ast::SwitchArm,
    context: &ComptimeLowerContext<'_>,
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
    context: &ComptimeLowerContext<'_>,
) -> Result<ComptimeSwitchPattern, ComptimeLowerError> {
    match pattern {
        nia_ast::SwitchPattern::Default => Ok(ComptimeSwitchPattern::Default),
        nia_ast::SwitchPattern::OptionalSome { name, span } => {
            Ok(ComptimeSwitchPattern::OptionalSome {
                name: name.clone(),
                local_id: context.local_id.and_then(|local_id| local_id(*span)),
                span: *span,
            })
        }
        nia_ast::SwitchPattern::OptionalNull { span } => {
            Ok(ComptimeSwitchPattern::OptionalNull { span: *span })
        }
        nia_ast::SwitchPattern::ErrorOk { name, span } => Ok(ComptimeSwitchPattern::ErrorOk {
            name: name.clone(),
            local_id: context.local_id.and_then(|local_id| local_id(*span)),
            span: *span,
        }),
        nia_ast::SwitchPattern::ErrorErr { name, span } => Ok(ComptimeSwitchPattern::ErrorErr {
            name: name.clone(),
            local_id: context.local_id.and_then(|local_id| local_id(*span)),
            span: *span,
        }),
        nia_ast::SwitchPattern::Expr(expr) => {
            lower_expr_with_context(expr, context).map(ComptimeSwitchPattern::Expr)
        }
        nia_ast::SwitchPattern::Range {
            start,
            end,
            inclusive,
            span,
        } => Ok(ComptimeSwitchPattern::Range {
            start: lower_expr_with_context(start, context)?,
            end: lower_expr_with_context(end, context)?,
            inclusive: *inclusive,
            span: *span,
        }),
    }
}

fn lower_switch_arm_body_with_context(
    body: &nia_ast::SwitchArmBody,
    context: &ComptimeLowerContext<'_>,
) -> Result<ComptimeSwitchArmBody, ComptimeLowerError> {
    match body {
        nia_ast::SwitchArmBody::Expr(expr) => {
            lower_expr_with_context(expr, context).map(ComptimeSwitchArmBody::Expr)
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
    context: &ComptimeLowerContext<'_>,
) -> Result<ComptimeFieldInit, ComptimeLowerError> {
    Ok(ComptimeFieldInit {
        span: field.span,
        name: field.name.clone(),
        value: lower_expr_with_context(&field.value, context)?,
    })
}
