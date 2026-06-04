// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{BinaryOp, StringLiteral, UnaryOp};
use nia_span::Span;

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
    Ident(String),
    Qualified {
        name: String,
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
    let kind = match &expr.kind {
        nia_ast::ExprKind::Integer(text) => ComptimeExprKind::Integer(text.clone()),
        nia_ast::ExprKind::String(literal) => ComptimeExprKind::String(literal.clone()),
        nia_ast::ExprKind::Bool(value) => ComptimeExprKind::Bool(*value),
        nia_ast::ExprKind::Ident(name) => ComptimeExprKind::Ident(name.clone()),
        nia_ast::ExprKind::Qualified { name, .. } => {
            ComptimeExprKind::Qualified { name: name.clone() }
        }
        nia_ast::ExprKind::Field { lhs, name } => ComptimeExprKind::Field {
            lhs: Box::new(lower_expr(lhs)?),
            name: name.clone(),
        },
        nia_ast::ExprKind::StructLiteral { fields }
        | nia_ast::ExprKind::TypedStructLiteral { fields, .. } => ComptimeExprKind::StructLiteral {
            fields: fields
                .iter()
                .map(lower_field_init)
                .collect::<Result<Vec<_>, _>>()?,
        },
        nia_ast::ExprKind::Builtin { name, type_arg } => ComptimeExprKind::Builtin {
            name: name.clone(),
            type_arg_span: type_arg.as_ref().map(|ty| ty.span),
        },
        nia_ast::ExprKind::Call { callee, args } => ComptimeExprKind::Call {
            callee: Box::new(lower_expr(callee)?),
            args: args.iter().map(lower_expr).collect::<Result<Vec<_>, _>>()?,
        },
        nia_ast::ExprKind::Unary { op, expr } => ComptimeExprKind::Unary {
            op: *op,
            expr: Box::new(lower_expr(expr)?),
        },
        nia_ast::ExprKind::Binary { lhs, op, rhs } => ComptimeExprKind::Binary {
            lhs: Box::new(lower_expr(lhs)?),
            op: *op,
            rhs: Box::new(lower_expr(rhs)?),
        },
        nia_ast::ExprKind::Cast { expr, .. } => ComptimeExprKind::Cast {
            expr: Box::new(lower_expr(expr)?),
        },
        nia_ast::ExprKind::Block(block) => ComptimeExprKind::Block(lower_block(block)?),
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

pub fn lower_function(
    function_span: Span,
    function: &nia_ast::FunctionItem,
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
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ComptimeFunction {
        span: function_span,
        params,
        body: lower_block(body)?,
    })
}

fn lower_block(block: &nia_ast::Block) -> Result<ComptimeBlock, ComptimeLowerError> {
    Ok(ComptimeBlock {
        span: block.span,
        stmts: block
            .stmts
            .iter()
            .map(lower_stmt)
            .collect::<Result<Vec<_>, _>>()?,
        tail: block
            .tail
            .as_deref()
            .map(lower_expr)
            .transpose()?
            .map(Box::new),
    })
}

fn lower_stmt(stmt: &nia_ast::Stmt) -> Result<ComptimeStmt, ComptimeLowerError> {
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
                value: lower_expr(value)?,
            })
        }
        nia_ast::StmtKind::Return(value) => {
            ComptimeStmtKind::Return(value.as_ref().map(lower_expr).transpose()?)
        }
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

fn lower_field_init(field: &nia_ast::FieldInit) -> Result<ComptimeFieldInit, ComptimeLowerError> {
    Ok(ComptimeFieldInit {
        span: field.span,
        name: field.name.clone(),
        value: lower_expr(&field.value)?,
    })
}
