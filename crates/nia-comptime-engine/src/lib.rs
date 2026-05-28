// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{BinaryOp, Expr, ExprKind, UnaryOp};
use nia_span::Span;

#[derive(Debug, Clone, PartialEq)]
pub enum ComptimeValue {
    Int(i128),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComptimeError {
    pub span: Span,
    pub message: String,
}

pub trait ComptimeEnv {
    fn resolve_ident(&mut self, span: Span, name: &str) -> Result<ComptimeValue, ComptimeError>;
}

#[derive(Default)]
pub struct EmptyEnv;

impl ComptimeEnv for EmptyEnv {
    fn resolve_ident(&mut self, span: Span, name: &str) -> Result<ComptimeValue, ComptimeError> {
        Err(ComptimeError {
            span,
            message: format!("unknown comptime value `{name}`"),
        })
    }
}

pub fn eval_expr(expr: &Expr, env: &mut impl ComptimeEnv) -> Result<ComptimeValue, ComptimeError> {
    match &expr.kind {
        ExprKind::Integer(text) => {
            eval_int_literal(text)
                .map(ComptimeValue::Int)
                .map_err(|message| ComptimeError {
                    span: expr.span,
                    message,
                })
        }
        ExprKind::Ident(name) => env.resolve_ident(expr.span, name),
        ExprKind::Qualified { name, .. } => env.resolve_ident(expr.span, name),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr: inner,
        } => {
            match eval_expr(inner, env)? {
                ComptimeValue::Int(value) => value
                    .checked_neg()
                    .map(ComptimeValue::Int)
                    .ok_or_else(|| ComptimeError {
                        span: expr.span,
                        message: "integer overflow in comptime negation".to_string(),
                    }),
            }
        }
        ExprKind::Unary { op, .. } => Err(ComptimeError {
            span: expr.span,
            message: format!("unsupported unary operator in comptime expression: {op:?}"),
        }),
        ExprKind::Binary { lhs, op, rhs } => {
            let ComptimeValue::Int(lhs) = eval_expr(lhs, env)?;
            let ComptimeValue::Int(rhs) = eval_expr(rhs, env)?;
            eval_binary_int(lhs, *op, rhs)
                .map(ComptimeValue::Int)
                .map_err(|message| ComptimeError {
                    span: expr.span,
                    message,
                })
        }
        ExprKind::Cast { expr: inner, .. } => eval_expr(inner, env),
        _ => Err(ComptimeError {
            span: expr.span,
            message: "unsupported comptime expression".to_string(),
        }),
    }
}

pub fn eval_int_expr(expr: &Expr, env: &mut impl ComptimeEnv) -> Result<i128, ComptimeError> {
    match eval_expr(expr, env)? {
        ComptimeValue::Int(value) => Ok(value),
    }
}

pub fn eval_array_len_expr(expr: &Expr, env: &mut impl ComptimeEnv) -> Result<u64, ComptimeError> {
    int_to_array_len(expr.span, eval_int_expr(expr, env)?)
}

pub fn eval_int_literal(text: &str) -> Result<i128, String> {
    parse_int_literal(text)
}

fn int_to_array_len(span: Span, value: i128) -> Result<u64, ComptimeError> {
    if value < 0 {
        return Err(ComptimeError {
            span,
            message: "array length must be non-negative".to_string(),
        });
    }
    u64::try_from(value).map_err(|_| ComptimeError {
        span,
        message: "array length is too large".to_string(),
    })
}

fn eval_binary_int(lhs: i128, op: BinaryOp, rhs: i128) -> Result<i128, String> {
    Ok(match op {
        BinaryOp::Mul => lhs
            .checked_mul(rhs)
            .ok_or_else(|| "integer overflow in comptime multiplication".to_string())?,
        BinaryOp::Div => {
            if rhs == 0 {
                return Err("division by zero in comptime expression".to_string());
            }
            lhs.checked_div(rhs)
                .ok_or_else(|| "integer overflow in comptime division".to_string())?
        }
        BinaryOp::Rem => {
            if rhs == 0 {
                return Err("remainder by zero in comptime expression".to_string());
            }
            lhs.checked_rem(rhs)
                .ok_or_else(|| "integer overflow in comptime remainder".to_string())?
        }
        BinaryOp::Add => lhs
            .checked_add(rhs)
            .ok_or_else(|| "integer overflow in comptime addition".to_string())?,
        BinaryOp::Sub => lhs
            .checked_sub(rhs)
            .ok_or_else(|| "integer overflow in comptime subtraction".to_string())?,
        BinaryOp::Shl => checked_shift(lhs, rhs, true)?,
        BinaryOp::Shr => checked_shift(lhs, rhs, false)?,
        BinaryOp::BitAnd => lhs & rhs,
        BinaryOp::BitXor => lhs ^ rhs,
        BinaryOp::BitOr => lhs | rhs,
        _ => {
            return Err(format!(
                "unsupported binary operator in comptime expression: {op:?}"
            ));
        }
    })
}

fn checked_shift(lhs: i128, rhs: i128, is_left: bool) -> Result<i128, String> {
    let Ok(rhs) = u32::try_from(rhs) else {
        return Err("shift count is out of range in comptime expression".to_string());
    };
    if rhs >= i128::BITS {
        return Err("shift count is out of range in comptime expression".to_string());
    }
    if is_left {
        lhs.checked_shl(rhs)
            .ok_or_else(|| "integer overflow in comptime left shift".to_string())
    } else {
        lhs.checked_shr(rhs)
            .ok_or_else(|| "integer overflow in comptime right shift".to_string())
    }
}

fn parse_int_literal(text: &str) -> Result<i128, String> {
    let (radix, digits) =
        if let Some(rest) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            (16, rest)
        } else if let Some(rest) = text.strip_prefix("0b").or_else(|| text.strip_prefix("0B")) {
            (2, rest)
        } else if let Some(rest) = text.strip_prefix("0o").or_else(|| text.strip_prefix("0O")) {
            (8, rest)
        } else {
            (10, text)
        };
    let digits = digits.replace('_', "");
    if digits.is_empty() {
        return Err("invalid integer constant".to_string());
    }
    i128::from_str_radix(&digits, radix)
        .map_err(|_| "integer literal is out of range for comptime evaluation".to_string())
}
