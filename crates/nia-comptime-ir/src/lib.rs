// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{BinaryOp, StringLiteral, UnaryOp};
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
    Return(Option<ComptimeExpr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeBinding {
    pub span: Span,
    pub name: String,
    pub local_id: Option<LocalId>,
    pub value: ComptimeExpr,
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
    Binary {
        lhs: Box<ComptimeExpr>,
        op: BinaryOp,
        rhs: Box<ComptimeExpr>,
    },
    Cast {
        expr: Box<ComptimeExpr>,
    },
    Block(ComptimeBlock),
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
        nia_ast::ExprKind::Binary { lhs, op, rhs } => ComptimeExprKind::Binary {
            lhs: Box::new(lower_expr_with_context(lhs, context)?),
            op: *op,
            rhs: Box::new(lower_expr_with_context(rhs, context)?),
        },
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
                value: lower_expr_with_context(value, context)?,
            })
        }
        nia_ast::StmtKind::Return(value) => ComptimeStmtKind::Return(
            value
                .as_ref()
                .map(|value| lower_expr_with_context(value, context))
                .transpose()?,
        ),
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
