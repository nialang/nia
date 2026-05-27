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

pub fn eval_int_text(text: &str) -> Option<i128> {
    eval_int_text_result(text).ok()
}

pub fn eval_int_text_result(text: &str) -> Result<i128, String> {
    Parser::new(text).parse_expr()
}

pub fn eval_array_len_text(text: &str) -> Result<u64, String> {
    let value = eval_int_text_result(text)?;
    int_to_array_len(Span::default(), value).map_err(|err| err.message)
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

struct Parser<'a> {
    source: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source: source.as_bytes(),
            pos: 0,
        }
    }

    fn parse_expr(&mut self) -> Result<i128, String> {
        let value = self.parse_bit_or()?;
        self.skip_ws();
        if self.pos == self.source.len() {
            Ok(value)
        } else {
            Err(self.error("unexpected trailing input in comptime expression"))
        }
    }

    fn parse_bit_or(&mut self) -> Result<i128, String> {
        let mut lhs = self.parse_bit_xor()?;
        loop {
            self.skip_ws();
            if !self.eat(b'|') {
                return Ok(lhs);
            }
            let rhs = self.parse_bit_xor()?;
            lhs |= rhs;
        }
    }

    fn parse_bit_xor(&mut self) -> Result<i128, String> {
        let mut lhs = self.parse_bit_and()?;
        loop {
            self.skip_ws();
            if !self.eat(b'^') {
                return Ok(lhs);
            }
            let rhs = self.parse_bit_and()?;
            lhs ^= rhs;
        }
    }

    fn parse_bit_and(&mut self) -> Result<i128, String> {
        let mut lhs = self.parse_shift()?;
        loop {
            self.skip_ws();
            if !self.eat(b'&') {
                return Ok(lhs);
            }
            let rhs = self.parse_shift()?;
            lhs &= rhs;
        }
    }

    fn parse_shift(&mut self) -> Result<i128, String> {
        let mut lhs = self.parse_add_sub()?;
        loop {
            self.skip_ws();
            if self.eat_two(b'<', b'<') {
                let rhs = self.parse_add_sub()?;
                lhs = checked_shift(lhs, rhs, true)?;
            } else if self.eat_two(b'>', b'>') {
                let rhs = self.parse_add_sub()?;
                lhs = checked_shift(lhs, rhs, false)?;
            } else {
                return Ok(lhs);
            }
        }
    }

    fn parse_add_sub(&mut self) -> Result<i128, String> {
        let mut lhs = self.parse_mul_div_rem()?;
        loop {
            self.skip_ws();
            if self.eat(b'+') {
                let rhs = self.parse_mul_div_rem()?;
                lhs = lhs
                    .checked_add(rhs)
                    .ok_or_else(|| self.error("integer overflow in comptime addition"))?;
            } else if self.eat(b'-') {
                let rhs = self.parse_mul_div_rem()?;
                lhs = lhs
                    .checked_sub(rhs)
                    .ok_or_else(|| self.error("integer overflow in comptime subtraction"))?;
            } else {
                return Ok(lhs);
            }
        }
    }

    fn parse_mul_div_rem(&mut self) -> Result<i128, String> {
        let mut lhs = self.parse_unary()?;
        loop {
            self.skip_ws();
            if self.eat(b'*') {
                let rhs = self.parse_unary()?;
                lhs = lhs
                    .checked_mul(rhs)
                    .ok_or_else(|| self.error("integer overflow in comptime multiplication"))?;
            } else if self.eat(b'/') {
                let rhs = self.parse_unary()?;
                if rhs == 0 {
                    return Err(self.error("division by zero in comptime expression"));
                }
                lhs = lhs
                    .checked_div(rhs)
                    .ok_or_else(|| self.error("integer overflow in comptime division"))?;
            } else if self.eat(b'%') {
                let rhs = self.parse_unary()?;
                if rhs == 0 {
                    return Err(self.error("remainder by zero in comptime expression"));
                }
                lhs = lhs
                    .checked_rem(rhs)
                    .ok_or_else(|| self.error("integer overflow in comptime remainder"))?;
            } else {
                return Ok(lhs);
            }
        }
    }

    fn parse_unary(&mut self) -> Result<i128, String> {
        self.skip_ws();
        if self.eat(b'-') {
            let value = self.parse_unary()?;
            return value
                .checked_neg()
                .ok_or_else(|| self.error("integer overflow in comptime negation"));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<i128, String> {
        self.skip_ws();
        if self.eat(b'(') {
            let value = self.parse_bit_or()?;
            self.skip_ws();
            if !self.eat(b')') {
                return Err(self.error("expected `)` in comptime expression"));
            }
            return Ok(value);
        }
        let start = self.pos;
        while self
            .source
            .get(self.pos)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(self.error("expected integer in comptime expression"));
        }
        let text = std::str::from_utf8(&self.source[start..self.pos]).unwrap_or("");
        parse_int_literal(text)
    }

    fn skip_ws(&mut self) {
        while self
            .source
            .get(self.pos)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            self.pos += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.source.get(self.pos) == Some(&byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn eat_two(&mut self, first: u8, second: u8) -> bool {
        if self.source.get(self.pos) == Some(&first)
            && self.source.get(self.pos + 1) == Some(&second)
        {
            self.pos += 2;
            true
        } else {
            false
        }
    }

    fn error(&self, message: &str) -> String {
        message.to_string()
    }
}
