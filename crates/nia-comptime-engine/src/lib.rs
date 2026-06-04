// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{BinaryOp, Expr, FunctionItem, UnaryOp};
pub use nia_comptime_ir::{
    ComptimeBinding, ComptimeBlock, ComptimeExpr, ComptimeExprKind, ComptimeFunction,
    ComptimeLowerError, ComptimeParam, ComptimeStmt, ComptimeStmtKind,
};
use nia_ids::LayoutBuiltin;
use nia_span::Span;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum ComptimeValue {
    Int(i128),
    Bool(bool),
    String(String),
    Struct(BTreeMap<String, ComptimeValue>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComptimeError {
    pub span: Span,
    pub message: String,
}

pub trait ComptimeEnv {
    fn resolve_ident(&mut self, span: Span, name: &str) -> Result<ComptimeValue, ComptimeError>;

    fn resolve_builtin_value(
        &mut self,
        span: Span,
        name: &str,
    ) -> Result<ComptimeValue, ComptimeError> {
        Err(ComptimeError {
            span,
            message: format!("unsupported builtin value in comptime expression: @{name}"),
        })
    }

    fn resolve_layout_builtin(
        &mut self,
        span: Span,
        builtin: LayoutBuiltin,
        type_arg_span: Span,
    ) -> Result<ComptimeValue, ComptimeError>;

    fn call_function(
        &mut self,
        span: Span,
        callee: &ComptimeExpr,
        args: Vec<ComptimeValue>,
    ) -> Result<ComptimeValue, ComptimeError> {
        let _ = callee;
        let _ = args;
        Err(ComptimeError {
            span,
            message: "unsupported comptime function call".to_string(),
        })
    }

    fn push_function_frame(&mut self, span: Span) -> Result<(), ComptimeError> {
        Err(ComptimeError {
            span,
            message: "comptime function calls are not available in this context".to_string(),
        })
    }

    fn pop_function_frame(&mut self) {}

    fn bind_function_param(
        &mut self,
        span: Span,
        param: &ComptimeParam,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let _ = param;
        let _ = value;
        Err(ComptimeError {
            span,
            message: "comptime function parameters are not available in this context".to_string(),
        })
    }

    fn bind_function_local(
        &mut self,
        span: Span,
        binding: &ComptimeBinding,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let _ = binding;
        let _ = value;
        Err(ComptimeError {
            span,
            message: "comptime function locals are not available in this context".to_string(),
        })
    }
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

    fn resolve_layout_builtin(
        &mut self,
        span: Span,
        _builtin: LayoutBuiltin,
        _type_arg_span: Span,
    ) -> Result<ComptimeValue, ComptimeError> {
        Err(ComptimeError {
            span,
            message: "layout builtins are not available in this comptime context".to_string(),
        })
    }
}

pub fn eval_expr(expr: &Expr, env: &mut impl ComptimeEnv) -> Result<ComptimeValue, ComptimeError> {
    let expr = nia_comptime_ir::lower_expr(expr).map_err(lower_error)?;
    eval_comptime_expr(&expr, env)
}

pub fn eval_comptime_expr(
    expr: &ComptimeExpr,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    match &expr.kind {
        ComptimeExprKind::Bool(value) => Ok(ComptimeValue::Bool(*value)),
        ComptimeExprKind::String(literal) => literal_string(literal)
            .map(ComptimeValue::String)
            .ok_or_else(|| ComptimeError {
                span: expr.span,
                message: "unsupported string literal in comptime expression".to_string(),
            }),
        ComptimeExprKind::Integer(text) => {
            eval_int_literal(text)
                .map(ComptimeValue::Int)
                .map_err(|message| ComptimeError {
                    span: expr.span,
                    message,
                })
        }
        ComptimeExprKind::Ident(name) => env.resolve_ident(expr.span, name),
        ComptimeExprKind::Qualified { name } => env.resolve_ident(expr.span, name),
        ComptimeExprKind::Field { lhs, name } => match eval_comptime_expr(lhs, env)? {
            ComptimeValue::Struct(fields) => {
                fields.get(name).cloned().ok_or_else(|| ComptimeError {
                    span: expr.span,
                    message: format!("unknown comptime field `{name}`"),
                })
            }
            _ => Err(ComptimeError {
                span: expr.span,
                message: "comptime field access requires a struct value".to_string(),
            }),
        },
        ComptimeExprKind::StructLiteral { fields } => eval_struct_literal(fields, env),
        ComptimeExprKind::Builtin {
            name,
            type_arg_span: Some(type_arg_span),
        } => {
            let Some(builtin) = LayoutBuiltin::from_name(name) else {
                return Err(ComptimeError {
                    span: expr.span,
                    message: format!("unsupported builtin in comptime expression: @{name}"),
                });
            };
            env.resolve_layout_builtin(expr.span, builtin, *type_arg_span)
        }
        ComptimeExprKind::Builtin {
            name,
            type_arg_span: None,
        } => env.resolve_builtin_value(expr.span, name),
        ComptimeExprKind::Call { callee, args } => {
            if let ComptimeExprKind::Builtin {
                name,
                type_arg_span,
            } = &callee.kind
            {
                if !args.is_empty() {
                    return Err(ComptimeError {
                        span: expr.span,
                        message: format!(
                            "unsupported builtin call in comptime expression: @{name}"
                        ),
                    });
                }
                if let Some(type_arg_span) = type_arg_span {
                    let Some(builtin) = LayoutBuiltin::from_name(name) else {
                        return Err(ComptimeError {
                            span: expr.span,
                            message: format!("unsupported builtin in comptime expression: @{name}"),
                        });
                    };
                    env.resolve_layout_builtin(expr.span, builtin, *type_arg_span)
                } else {
                    env.resolve_builtin_value(expr.span, name)
                }
            } else {
                let args = args
                    .iter()
                    .map(|arg| eval_comptime_expr(arg, env))
                    .collect::<Result<Vec<_>, _>>()?;
                env.call_function(expr.span, callee, args)
            }
        }
        ComptimeExprKind::Unary {
            op: UnaryOp::Neg,
            expr: inner,
        } => {
            match eval_comptime_expr(inner, env)? {
                ComptimeValue::Int(value) => value
                    .checked_neg()
                    .map(ComptimeValue::Int)
                    .ok_or_else(|| ComptimeError {
                        span: expr.span,
                        message: "integer overflow in comptime negation".to_string(),
                    }),
                _ => Err(ComptimeError {
                    span: expr.span,
                    message: "comptime negation requires an integer".to_string(),
                }),
            }
        }
        ComptimeExprKind::Unary {
            op: UnaryOp::Not,
            expr: inner,
        } => match eval_comptime_expr(inner, env)? {
            ComptimeValue::Bool(value) => Ok(ComptimeValue::Bool(!value)),
            _ => Err(ComptimeError {
                span: expr.span,
                message: "comptime `not` requires a bool".to_string(),
            }),
        },
        ComptimeExprKind::Unary { op, .. } => Err(ComptimeError {
            span: expr.span,
            message: format!("unsupported unary operator in comptime expression: {op:?}"),
        }),
        ComptimeExprKind::Binary { lhs, op, rhs } => eval_binary(expr.span, lhs, *op, rhs, env),
        ComptimeExprKind::Cast { expr: inner } => eval_comptime_expr(inner, env),
        ComptimeExprKind::Block(block) => {
            if !block.stmts.is_empty() {
                return Err(ComptimeError {
                    span: expr.span,
                    message: "comptime expression block cannot contain statements".to_string(),
                });
            }
            let Some(tail) = &block.tail else {
                return Err(ComptimeError {
                    span: expr.span,
                    message: "comptime expression block requires a tail expression".to_string(),
                });
            };
            eval_comptime_expr(tail, env)
        }
    }
}

fn eval_struct_literal(
    fields: &[nia_comptime_ir::ComptimeFieldInit],
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    let mut values = BTreeMap::new();
    for field in fields {
        if values
            .insert(field.name.clone(), eval_comptime_expr(&field.value, env)?)
            .is_some()
        {
            return Err(ComptimeError {
                span: field.span,
                message: format!("duplicate comptime struct field `{}`", field.name),
            });
        }
    }
    Ok(ComptimeValue::Struct(values))
}

pub fn eval_int_expr(expr: &Expr, env: &mut impl ComptimeEnv) -> Result<i128, ComptimeError> {
    let expr = nia_comptime_ir::lower_expr(expr).map_err(lower_error)?;
    eval_comptime_int_expr(&expr, env)
}

pub fn eval_comptime_int_expr(
    expr: &ComptimeExpr,
    env: &mut impl ComptimeEnv,
) -> Result<i128, ComptimeError> {
    match eval_comptime_expr(expr, env)? {
        ComptimeValue::Int(value) => Ok(value),
        _ => Err(ComptimeError {
            span: expr.span,
            message: "comptime expression must evaluate to an integer".to_string(),
        }),
    }
}

pub fn eval_bool_expr(expr: &Expr, env: &mut impl ComptimeEnv) -> Result<bool, ComptimeError> {
    let expr = nia_comptime_ir::lower_expr(expr).map_err(lower_error)?;
    eval_comptime_bool_expr(&expr, env)
}

pub fn eval_comptime_bool_expr(
    expr: &ComptimeExpr,
    env: &mut impl ComptimeEnv,
) -> Result<bool, ComptimeError> {
    match eval_comptime_expr(expr, env)? {
        ComptimeValue::Bool(value) => Ok(value),
        _ => Err(ComptimeError {
            span: expr.span,
            message: "comptime expression must evaluate to bool".to_string(),
        }),
    }
}

pub fn eval_array_len_expr(expr: &Expr, env: &mut impl ComptimeEnv) -> Result<u64, ComptimeError> {
    let expr = nia_comptime_ir::lower_expr(expr).map_err(lower_error)?;
    eval_comptime_array_len_expr(&expr, env)
}

pub fn eval_comptime_array_len_expr(
    expr: &ComptimeExpr,
    env: &mut impl ComptimeEnv,
) -> Result<u64, ComptimeError> {
    int_to_array_len(expr.span, eval_comptime_int_expr(expr, env)?)
}

pub fn eval_function_call(
    span: Span,
    function_span: Span,
    function: &FunctionItem,
    args: Vec<ComptimeValue>,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    let function = nia_comptime_ir::lower_function(function_span, function).map_err(lower_error)?;
    eval_comptime_function_call(span, &function, args, env)
}

pub fn eval_comptime_function_call(
    span: Span,
    function: &ComptimeFunction,
    args: Vec<ComptimeValue>,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    if function.params.len() != args.len() {
        return Err(ComptimeError {
            span,
            message: format!(
                "comptime function argument count mismatch: expected {}, got {}",
                function.params.len(),
                args.len()
            ),
        });
    }
    env.push_function_frame(span)?;
    for (param, value) in function.params.iter().zip(args) {
        if let Err(err) = env.bind_function_param(param.span, param, value) {
            env.pop_function_frame();
            return Err(err);
        }
    }
    let result = eval_function_block(&function.body, env).and_then(|value| {
        value.ok_or_else(|| ComptimeError {
            span: function.body.span,
            message: "comptime function must return a value".to_string(),
        })
    });
    env.pop_function_frame();
    result
}

fn eval_function_block(
    block: &ComptimeBlock,
    env: &mut impl ComptimeEnv,
) -> Result<Option<ComptimeValue>, ComptimeError> {
    for stmt in &block.stmts {
        if let Some(value) = eval_function_stmt(stmt, env)? {
            return Ok(Some(value));
        }
    }
    block
        .tail
        .as_deref()
        .map_or(Ok(None), |tail| eval_comptime_expr(tail, env).map(Some))
}

fn eval_function_stmt(
    stmt: &ComptimeStmt,
    env: &mut impl ComptimeEnv,
) -> Result<Option<ComptimeValue>, ComptimeError> {
    match &stmt.kind {
        ComptimeStmtKind::Binding(binding) => {
            let value = eval_comptime_expr(&binding.value, env)?;
            env.bind_function_local(stmt.span, binding, value)?;
            Ok(None)
        }
        ComptimeStmtKind::Return(value) => {
            let Some(value) = value else {
                return Err(ComptimeError {
                    span: stmt.span,
                    message: "comptime function must return a value".to_string(),
                });
            };
            eval_comptime_expr(value, env).map(Some)
        }
    }
}

pub fn eval_int_literal(text: &str) -> Result<i128, String> {
    parse_int_literal(text)
}

pub fn eval_float_literal(text: &str) -> Result<f64, String> {
    let body = numeric_literal_body(text);
    body.replace('_', "")
        .parse::<f64>()
        .map_err(|_| "invalid float constant".to_string())
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

fn eval_binary(
    span: Span,
    lhs: &ComptimeExpr,
    op: BinaryOp,
    rhs: &ComptimeExpr,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    match op {
        BinaryOp::And => {
            let lhs = eval_comptime_bool_expr(lhs, env)?;
            if !lhs {
                return Ok(ComptimeValue::Bool(false));
            }
            eval_comptime_bool_expr(rhs, env).map(ComptimeValue::Bool)
        }
        BinaryOp::Or => {
            let lhs = eval_comptime_bool_expr(lhs, env)?;
            if lhs {
                return Ok(ComptimeValue::Bool(true));
            }
            eval_comptime_bool_expr(rhs, env).map(ComptimeValue::Bool)
        }
        BinaryOp::Eq | BinaryOp::Ne => {
            let lhs = eval_comptime_expr(lhs, env)?;
            let rhs = eval_comptime_expr(rhs, env)?;
            let equal = values_equal(&lhs, &rhs).ok_or_else(|| ComptimeError {
                span,
                message: "comptime equality requires matching operand types".to_string(),
            })?;
            Ok(ComptimeValue::Bool(if op == BinaryOp::Eq {
                equal
            } else {
                !equal
            }))
        }
        _ => {
            let lhs = eval_comptime_int_expr(lhs, env)?;
            let rhs = eval_comptime_int_expr(rhs, env)?;
            eval_binary_int(lhs, op, rhs)
                .map(ComptimeValue::Int)
                .map_err(|message| ComptimeError { span, message })
        }
    }
}

fn lower_error(err: ComptimeLowerError) -> ComptimeError {
    ComptimeError {
        span: err.span,
        message: err.message,
    }
}

fn values_equal(lhs: &ComptimeValue, rhs: &ComptimeValue) -> Option<bool> {
    match (lhs, rhs) {
        (ComptimeValue::Int(lhs), ComptimeValue::Int(rhs)) => Some(lhs == rhs),
        (ComptimeValue::Bool(lhs), ComptimeValue::Bool(rhs)) => Some(lhs == rhs),
        (ComptimeValue::String(lhs), ComptimeValue::String(rhs)) => Some(lhs == rhs),
        _ => None,
    }
}

fn literal_string(literal: &nia_ast::StringLiteral) -> Option<String> {
    if literal.parts.len() != 1 {
        return None;
    }
    let text = literal.parts[0].as_str();
    text.strip_prefix('"')?
        .strip_suffix('"')
        .map(unescape_simple)
}

fn unescape_simple(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('0') => out.push('\0'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
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
    let text = numeric_literal_body(text);
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

fn numeric_literal_body(text: &str) -> &str {
    let suffix_start = numeric_suffix_start(text).unwrap_or(text.len());
    &text[..suffix_start]
}

fn numeric_suffix_start(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let non_decimal_radix = text.starts_with("0x")
        || text.starts_with("0X")
        || text.starts_with("0b")
        || text.starts_with("0B")
        || text.starts_with("0o")
        || text.starts_with("0O");
    let mut index = if non_decimal_radix { 2 } else { 0 };
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'_'
            || if non_decimal_radix {
                digit_value(byte).is_some()
            } else {
                byte.is_ascii_digit()
            }
        {
            index += 1;
        } else {
            break;
        }
    }
    if !non_decimal_radix && index < bytes.len() && bytes[index] == b'.' {
        index += 1;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == b'_' || byte.is_ascii_digit() {
                index += 1;
            } else {
                break;
            }
        }
    }
    if !non_decimal_radix && index < bytes.len() && matches!(bytes[index], b'e' | b'E') {
        index += 1;
        if index < bytes.len() && matches!(bytes[index], b'+' | b'-') {
            index += 1;
        }
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == b'_' || byte.is_ascii_digit() {
                index += 1;
            } else {
                break;
            }
        }
    }
    (index < bytes.len()).then_some(index)
}

fn digit_value(byte: u8) -> Option<u32> {
    match byte {
        b'0'..=b'9' => Some(u32::from(byte - b'0')),
        b'a'..=b'f' => Some(u32::from(byte - b'a') + 10),
        b'A'..=b'F' => Some(u32::from(byte - b'A') + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn eval_int_literal_ignores_type_suffix() {
        assert_eq!(eval_int_literal("42i32"), Ok(42));
        assert_eq!(eval_int_literal("0xffu8"), Ok(255));
        assert_eq!(eval_int_literal("1_024usize"), Ok(1024));
    }

    #[test]
    fn eval_float_literal_ignores_type_suffix_and_separators() {
        assert_eq!(eval_float_literal("0.0f64"), Ok(0.0));
        assert_eq!(eval_float_literal("1_024.5f32"), Ok(1024.5));
        assert_eq!(eval_float_literal("1.25e-1f64"), Ok(0.125));
    }

    #[test]
    fn evaluates_builtin_struct_field_conditions() {
        let (module, errors) = nia_parser::parse_module(
            r#"
fn main() bool {
    @builtin().target.os == "linux" and @builtin().target.pointer_width == 64
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
            panic!("expected function");
        };
        let expr = function.body.as_ref().unwrap().tail.as_deref().unwrap();
        let value = eval_bool_expr(expr, &mut BuiltinEnv).unwrap();
        assert!(value);
    }

    #[test]
    fn evaluates_lowered_comptime_expr_directly() {
        let (module, errors) = nia_parser::parse_module(
            r#"
fn main() bool {
    @builtin().target.os == "linux"
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
            panic!("expected function");
        };
        let expr = function.body.as_ref().unwrap().tail.as_deref().unwrap();
        let lowered = nia_comptime_ir::lower_expr(expr).unwrap();
        let ComptimeExprKind::Binary { lhs, .. } = &lowered.kind else {
            panic!("expected lowered binary expression");
        };
        let ComptimeExprKind::Field { name, .. } = &lhs.kind else {
            panic!("expected lowered field expression");
        };
        assert_eq!(name, "os");

        let value = eval_comptime_bool_expr(&lowered, &mut BuiltinEnv).unwrap();
        assert!(value);
    }

    struct BuiltinEnv;

    impl ComptimeEnv for BuiltinEnv {
        fn resolve_ident(
            &mut self,
            span: Span,
            name: &str,
        ) -> Result<ComptimeValue, ComptimeError> {
            Err(ComptimeError {
                span,
                message: format!("unknown comptime value `{name}`"),
            })
        }

        fn resolve_builtin_value(
            &mut self,
            span: Span,
            name: &str,
        ) -> Result<ComptimeValue, ComptimeError> {
            if name != "builtin" {
                return Err(ComptimeError {
                    span,
                    message: format!("unsupported builtin @{name}"),
                });
            }
            let mut target = BTreeMap::new();
            target.insert("os".to_string(), ComptimeValue::String("linux".to_string()));
            target.insert("pointer_width".to_string(), ComptimeValue::Int(64));
            let mut builtin = BTreeMap::new();
            builtin.insert("target".to_string(), ComptimeValue::Struct(target));
            Ok(ComptimeValue::Struct(builtin))
        }

        fn resolve_layout_builtin(
            &mut self,
            span: Span,
            _builtin: LayoutBuiltin,
            _type_arg_span: Span,
        ) -> Result<ComptimeValue, ComptimeError> {
            Err(ComptimeError {
                span,
                message: "layout builtins are not available in this test".to_string(),
            })
        }
    }
}
