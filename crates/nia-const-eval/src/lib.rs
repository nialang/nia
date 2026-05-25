// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{ArrayLen, BinaryOp, EnumItem, Expr, ExprKind, ItemKind, Module, TypeKind, UnaryOp};
use nia_ast_walk::{Visitor, walk_type};
use nia_defs::{DefCollection, DefId};
use nia_diagnostic::Diagnostic;
use nia_item_signatures::ItemSignatures;
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyInterner, TyKind};
use nia_type_lower::TypeLowering;

#[derive(Debug, Clone, PartialEq)]
pub struct ConstEval {
    pub enum_values: HashMap<DefId, ConstValue>,
    pub array_lengths: HashMap<String, ConstValue>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstValue {
    Int(i128),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstEvalError {
    pub message: String,
}

pub fn eval_module_consts(
    module: &Module,
    defs: &DefCollection,
    lowered: &TypeLowering,
    signatures: &ItemSignatures,
) -> ConstEval {
    let mut evaluator = ConstEvaluator {
        defs,
        interner: &lowered.interner,
        signatures,
        enum_values: HashMap::new(),
        array_lengths: HashMap::new(),
        diagnostics: Vec::new(),
    };
    evaluator.eval_module(module);
    evaluator.eval_array_lengths(module);
    ConstEval {
        enum_values: evaluator.enum_values,
        array_lengths: evaluator.array_lengths,
        diagnostics: evaluator.diagnostics,
    }
}

struct ConstEvaluator<'a> {
    defs: &'a DefCollection,
    interner: &'a TyInterner,
    signatures: &'a ItemSignatures,
    enum_values: HashMap<DefId, ConstValue>,
    array_lengths: HashMap<String, ConstValue>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> ConstEvaluator<'a> {
    fn eval_module(&mut self, module: &Module) {
        for item in &module.items {
            if let ItemKind::Enum(item_enum) = &item.kind {
                self.eval_enum(item.span, item_enum);
            }
        }
    }

    fn eval_enum(&mut self, item_span: Span, item_enum: &EnumItem) {
        let Some(enum_id) = self.defs.def_spans.get(item_span) else {
            return;
        };
        let range = self.enum_backing_range(enum_id);
        let mut next_value = 0i128;
        for variant in &item_enum.variants {
            let Some(variant_id) = self.defs.def_spans.get(variant.span) else {
                continue;
            };
            let value = if let Some(expr) = &variant.value {
                match self.eval_int_expr(expr) {
                    Some(ConstValue::Int(value)) => value,
                    None => {
                        next_value += 1;
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
            self.enum_values.insert(variant_id, ConstValue::Int(value));
            next_value = value.saturating_add(1);
        }
    }

    fn enum_backing_range(&self, enum_id: DefId) -> Option<(i128, i128)> {
        let signature = self.signatures.enums.get(&enum_id)?;
        let Some(TyKind::Primitive(primitive)) = self.interner.get(signature.backing_type) else {
            return None;
        };
        integer_range(*primitive)
    }

    fn eval_array_lengths(&mut self, module: &Module) {
        let mut collector = ArrayLenCollector {
            array_lengths: &mut self.array_lengths,
            diagnostics: &mut self.diagnostics,
        };
        nia_ast_walk::walk_module(&mut collector, module);
    }

    fn eval_int_expr(&mut self, expr: &Expr) -> Option<ConstValue> {
        match &expr.kind {
            ExprKind::Integer(text) => eval_int_literal(text)
                .map(ConstValue::Int)
                .map_err(|err| {
                    self.diagnostics
                        .push(Diagnostic::error(expr.span, err.message));
                })
                .ok(),
            ExprKind::Unary {
                op: UnaryOp::Neg,
                expr: inner,
            } => match self.eval_int_expr(inner)? {
                ConstValue::Int(value) => value.checked_neg().map(ConstValue::Int).or_else(|| {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        "integer overflow in constant negation",
                    ));
                    None
                }),
            },
            ExprKind::Unary { op, .. } => {
                self.diagnostics.push(Diagnostic::error(
                    expr.span,
                    format!("unsupported unary operator in constant expression: {op:?}"),
                ));
                None
            }
            ExprKind::Binary { lhs, op, rhs } => {
                let ConstValue::Int(lhs) = self.eval_int_expr(lhs)?;
                let ConstValue::Int(rhs) = self.eval_int_expr(rhs)?;
                eval_binary_int(lhs, *op, rhs)
                    .map(ConstValue::Int)
                    .map_err(|err| {
                        self.diagnostics
                            .push(Diagnostic::error(expr.span, err.message));
                    })
                    .ok()
            }
            ExprKind::Builtin { name, .. } if name == "size" || name == "align" => {
                self.diagnostics.push(Diagnostic::error(
                    expr.span,
                    format!(
                        "`@{name}` is not supported in this const-eval pass; it requires type layout"
                    ),
                ));
                None
            }
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    expr.span,
                    "unsupported constant expression",
                ));
                None
            }
        }
    }
}

struct ArrayLenCollector<'a> {
    array_lengths: &'a mut HashMap<String, ConstValue>,
    diagnostics: &'a mut Vec<Diagnostic>,
}

impl Visitor<'_> for ArrayLenCollector<'_> {
    fn visit_type(&mut self, ty: &nia_ast::TypeRef) {
        if let TypeKind::Array {
            len: ArrayLen::Expr(expr),
            ..
        } = &ty.kind
        {
            if is_layout_builtin_expr(expr) {
                walk_type(self, ty);
                return;
            }
            let text = expr_text(expr);
            if let std::collections::hash_map::Entry::Vacant(entry) = self.array_lengths.entry(text)
            {
                match eval_array_len_expr(expr) {
                    Ok(value) => {
                        entry.insert(ConstValue::Int(value as i128));
                    }
                    Err(err) => self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        format!("array length is not a valid constant: {}", err.message),
                    )),
                }
            }
        }
        walk_type(self, ty);
    }
}

fn is_layout_builtin_expr(expr: &Expr) -> bool {
    matches!(
        &expr.kind,
        ExprKind::Builtin { name, .. } if name == "size" || name == "align"
    )
}

fn expr_text(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Integer(text) | ExprKind::Raw(text) => text.clone(),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => format!("-{}", expr_text(expr)),
        ExprKind::Binary { lhs, op, rhs } => {
            format!(
                "{} {} {}",
                expr_text(lhs),
                binary_op_text(*op),
                expr_text(rhs)
            )
        }
        _ => "<const-expr>".to_string(),
    }
}

fn binary_op_text(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Shl => "<<",
        BinaryOp::Shr => ">>",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::BitAnd => "&",
        BinaryOp::BitXor => "^",
        BinaryOp::BitOr => "|",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
    }
}

pub fn eval_int_text(text: &str) -> Option<i128> {
    eval_int_text_result(text).ok()
}

pub fn eval_int_text_result(text: &str) -> Result<i128, ConstEvalError> {
    Parser::new(text).parse_expr()
}

pub fn eval_int_literal(text: &str) -> Result<i128, ConstEvalError> {
    parse_int_literal(text)
}

pub fn eval_array_len_text(text: &str) -> Result<u64, ConstEvalError> {
    let value = eval_int_text_result(text)?;
    int_to_array_len(value)
}

pub fn eval_array_len_expr(expr: &Expr) -> Result<u64, ConstEvalError> {
    match eval_int_expr_result(expr)? {
        ConstValue::Int(value) => int_to_array_len(value),
    }
}

pub fn eval_int_expr(expr: &Expr) -> Result<i128, ConstEvalError> {
    match eval_int_expr_result(expr)? {
        ConstValue::Int(value) => Ok(value),
    }
}

fn int_to_array_len(value: i128) -> Result<u64, ConstEvalError> {
    if value < 0 {
        return Err(ConstEvalError {
            message: "array length must be non-negative".to_string(),
        });
    }
    u64::try_from(value).map_err(|_| ConstEvalError {
        message: "array length is too large".to_string(),
    })
}

fn eval_int_expr_result(expr: &Expr) -> Result<ConstValue, ConstEvalError> {
    match &expr.kind {
        ExprKind::Integer(text) => eval_int_literal(text).map(ConstValue::Int),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr: inner,
        } => match eval_int_expr_result(inner)? {
            ConstValue::Int(value) => {
                value
                    .checked_neg()
                    .map(ConstValue::Int)
                    .ok_or_else(|| ConstEvalError {
                        message: "integer overflow in constant negation".to_string(),
                    })
            }
        },
        ExprKind::Unary { op, .. } => Err(ConstEvalError {
            message: format!("unsupported unary operator in constant expression: {op:?}"),
        }),
        ExprKind::Binary { lhs, op, rhs } => {
            let ConstValue::Int(lhs) = eval_int_expr_result(lhs)?;
            let ConstValue::Int(rhs) = eval_int_expr_result(rhs)?;
            eval_binary_int(lhs, *op, rhs).map(ConstValue::Int)
        }
        ExprKind::Builtin { name, .. } if name == "size" || name == "align" => {
            Err(ConstEvalError {
                message: format!(
                    "`@{name}` is not supported in this const-eval pass; it requires type layout"
                ),
            })
        }
        _ => Err(ConstEvalError {
            message: "unsupported constant expression".to_string(),
        }),
    }
}

fn eval_binary_int(lhs: i128, op: BinaryOp, rhs: i128) -> Result<i128, ConstEvalError> {
    Ok(match op {
        BinaryOp::Mul => lhs.checked_mul(rhs).ok_or_else(|| ConstEvalError {
            message: "integer overflow in constant multiplication".to_string(),
        })?,
        BinaryOp::Div => {
            if rhs == 0 {
                return Err(ConstEvalError {
                    message: "division by zero in constant expression".to_string(),
                });
            }
            lhs.checked_div(rhs).ok_or_else(|| ConstEvalError {
                message: "integer overflow in constant division".to_string(),
            })?
        }
        BinaryOp::Rem => {
            if rhs == 0 {
                return Err(ConstEvalError {
                    message: "remainder by zero in constant expression".to_string(),
                });
            }
            lhs.checked_rem(rhs).ok_or_else(|| ConstEvalError {
                message: "integer overflow in constant remainder".to_string(),
            })?
        }
        BinaryOp::Add => lhs.checked_add(rhs).ok_or_else(|| ConstEvalError {
            message: "integer overflow in constant addition".to_string(),
        })?,
        BinaryOp::Sub => lhs.checked_sub(rhs).ok_or_else(|| ConstEvalError {
            message: "integer overflow in constant subtraction".to_string(),
        })?,
        BinaryOp::Shl => checked_shift(lhs, rhs, true)?,
        BinaryOp::Shr => checked_shift(lhs, rhs, false)?,
        BinaryOp::BitAnd => lhs & rhs,
        BinaryOp::BitXor => lhs ^ rhs,
        BinaryOp::BitOr => lhs | rhs,
        _ => {
            return Err(ConstEvalError {
                message: format!("unsupported binary operator in constant expression: {op:?}"),
            });
        }
    })
}

fn checked_shift(lhs: i128, rhs: i128, is_left: bool) -> Result<i128, ConstEvalError> {
    let Ok(rhs) = u32::try_from(rhs) else {
        return Err(ConstEvalError {
            message: "shift count is out of range in constant expression".to_string(),
        });
    };
    if rhs >= i128::BITS {
        return Err(ConstEvalError {
            message: "shift count is out of range in constant expression".to_string(),
        });
    }
    if is_left {
        lhs.checked_shl(rhs).ok_or_else(|| ConstEvalError {
            message: "integer overflow in constant left shift".to_string(),
        })
    } else {
        lhs.checked_shr(rhs).ok_or_else(|| ConstEvalError {
            message: "integer overflow in constant right shift".to_string(),
        })
    }
}

fn integer_range(primitive: PrimitiveTy) -> Option<(i128, i128)> {
    Some(match primitive {
        PrimitiveTy::I8 => (i8::MIN as i128, i8::MAX as i128),
        PrimitiveTy::I16 => (i16::MIN as i128, i16::MAX as i128),
        PrimitiveTy::I32 => (i32::MIN as i128, i32::MAX as i128),
        PrimitiveTy::I64 => (i64::MIN as i128, i64::MAX as i128),
        PrimitiveTy::I128 => (i128::MIN, i128::MAX),
        PrimitiveTy::Isize => (isize::MIN as i128, isize::MAX as i128),
        PrimitiveTy::U8 => (u8::MIN as i128, u8::MAX as i128),
        PrimitiveTy::U16 => (u16::MIN as i128, u16::MAX as i128),
        PrimitiveTy::U32 => (u32::MIN as i128, u32::MAX as i128),
        PrimitiveTy::U64 => (u64::MIN as i128, u64::MAX as i128),
        PrimitiveTy::U128 => (0, i128::MAX),
        PrimitiveTy::Usize => (usize::MIN as i128, usize::MAX as i128),
        PrimitiveTy::F32
        | PrimitiveTy::F64
        | PrimitiveTy::Bool
        | PrimitiveTy::Char
        | PrimitiveTy::Void
        | PrimitiveTy::Never => return None,
    })
}

fn parse_int_literal(text: &str) -> Result<i128, ConstEvalError> {
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
        return Err(ConstEvalError {
            message: "invalid integer constant".to_string(),
        });
    }
    i128::from_str_radix(&digits, radix).map_err(|_| ConstEvalError {
        message: "integer literal is out of range for const evaluation".to_string(),
    })
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

    fn parse_expr(&mut self) -> Result<i128, ConstEvalError> {
        let value = self.parse_bit_or()?;
        self.skip_ws();
        if self.pos == self.source.len() {
            Ok(value)
        } else {
            Err(self.error("unexpected trailing input in constant expression"))
        }
    }

    fn parse_bit_or(&mut self) -> Result<i128, ConstEvalError> {
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

    fn parse_bit_xor(&mut self) -> Result<i128, ConstEvalError> {
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

    fn parse_bit_and(&mut self) -> Result<i128, ConstEvalError> {
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

    fn parse_shift(&mut self) -> Result<i128, ConstEvalError> {
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

    fn parse_add_sub(&mut self) -> Result<i128, ConstEvalError> {
        let mut lhs = self.parse_mul_div_rem()?;
        loop {
            self.skip_ws();
            if self.eat(b'+') {
                let rhs = self.parse_mul_div_rem()?;
                lhs = lhs
                    .checked_add(rhs)
                    .ok_or_else(|| self.error("integer overflow in constant addition"))?;
            } else if self.eat(b'-') {
                let rhs = self.parse_mul_div_rem()?;
                lhs = lhs
                    .checked_sub(rhs)
                    .ok_or_else(|| self.error("integer overflow in constant subtraction"))?;
            } else {
                return Ok(lhs);
            }
        }
    }

    fn parse_mul_div_rem(&mut self) -> Result<i128, ConstEvalError> {
        let mut lhs = self.parse_unary()?;
        loop {
            self.skip_ws();
            if self.eat(b'*') {
                let rhs = self.parse_unary()?;
                lhs = lhs
                    .checked_mul(rhs)
                    .ok_or_else(|| self.error("integer overflow in constant multiplication"))?;
            } else if self.eat(b'/') {
                let rhs = self.parse_unary()?;
                if rhs == 0 {
                    return Err(self.error("division by zero in constant expression"));
                }
                lhs = lhs
                    .checked_div(rhs)
                    .ok_or_else(|| self.error("integer overflow in constant division"))?;
            } else if self.eat(b'%') {
                let rhs = self.parse_unary()?;
                if rhs == 0 {
                    return Err(self.error("remainder by zero in constant expression"));
                }
                lhs = lhs
                    .checked_rem(rhs)
                    .ok_or_else(|| self.error("integer overflow in constant remainder"))?;
            } else {
                return Ok(lhs);
            }
        }
    }

    fn parse_unary(&mut self) -> Result<i128, ConstEvalError> {
        self.skip_ws();
        if self.eat(b'-') {
            self.parse_unary()?
                .checked_neg()
                .ok_or_else(|| self.error("integer overflow in constant negation"))
        } else if self.eat(b'+') {
            self.parse_unary()
        } else {
            self.parse_primary()
        }
    }

    fn parse_primary(&mut self) -> Result<i128, ConstEvalError> {
        self.skip_ws();
        if self.eat(b'(') {
            let value = self.parse_bit_or()?;
            self.skip_ws();
            if self.eat(b')') {
                Ok(value)
            } else {
                Err(self.error("expected `)` in constant expression"))
            }
        } else {
            self.parse_number()
        }
    }

    fn parse_number(&mut self) -> Result<i128, ConstEvalError> {
        self.skip_ws();
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            self.pos += 1;
        }
        if start == self.pos {
            return Err(self.error("expected integer literal in constant expression"));
        }
        let text = std::str::from_utf8(&self.source[start..self.pos])
            .map_err(|_| self.error("invalid utf-8 in constant expression integer literal"))?;
        parse_int_literal(text)
    }

    fn eat(&mut self, byte: u8) -> bool {
        self.skip_ws();
        if self.peek() == Some(byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn eat_two(&mut self, first: u8, second: u8) -> bool {
        self.skip_ws();
        if self.peek() == Some(first) && self.source.get(self.pos + 1) == Some(&second) {
            self.pos += 2;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.source.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    fn error(&self, message: impl Into<String>) -> ConstEvalError {
        ConstEvalError {
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{ModuleId, collect_module_defs};
    use nia_item_signatures::collect_item_signatures;
    use nia_parser::parse_module;
    use nia_type_lower::lower_module_types;
    use nia_type_resolve::resolve_module_types;

    #[test]
    fn evaluates_enum_values_and_array_lengths() {
        let (module, errors) = parse_module(
            r#"
enum Code: i32 {
    Ok = 0,
    NotFound = 400 + 4,
    Next,
}

fn main() [2 + 3 * 4]u8 {
    [0; 14]
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types(&module, &resolved);
        let signatures = collect_item_signatures(&module, &defs, &lowered);
        let eval = eval_module_consts(&module, &defs, &lowered, &signatures);
        assert!(eval.diagnostics.is_empty(), "{:?}", eval.diagnostics);
        assert!(
            eval.enum_values
                .values()
                .any(|value| *value == ConstValue::Int(404))
        );
        assert!(
            eval.enum_values
                .values()
                .any(|value| *value == ConstValue::Int(405))
        );
        assert_eq!(
            eval.array_lengths.get("2 + 3 * 4"),
            Some(&ConstValue::Int(14))
        );
    }

    #[test]
    fn rejects_non_literal_const_expressions() {
        let (module, errors) = parse_module(
            r#"
enum Code {
    Bad = missing,
}

fn main() [missing]u8 {
    [0]
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types(&module, &resolved);
        let signatures = collect_item_signatures(&module, &defs, &lowered);
        let eval = eval_module_consts(&module, &defs, &lowered, &signatures);
        assert!(eval.diagnostics.len() >= 2);
        assert!(eval.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("array length is not a valid constant")
                && !diagnostic.span.is_empty()
        }));
    }

    #[test]
    fn reports_enum_values_outside_backing_type_range() {
        let (module, errors) = parse_module(
            r#"
enum Tiny: u8 {
    Ok = 255,
    TooLarge,
}

enum SignedTiny: i8 {
    TooSmall = -129,
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types(&module, &resolved);
        let signatures = collect_item_signatures(&module, &defs, &lowered);
        let eval = eval_module_consts(&module, &defs, &lowered, &signatures);
        assert_eq!(
            eval.diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.message.contains("out of range"))
                .count(),
            2
        );
    }

    #[test]
    fn evaluates_text_integer_expressions() {
        assert_eq!(eval_int_text("1 + 2 * 3"), Some(7));
        assert_eq!(eval_int_text("(1 + 2) * 3"), Some(9));
        assert_eq!(eval_int_text("0b1010 | 0x10"), Some(26));
        assert_eq!(eval_int_text("1 << 4"), Some(16));
        assert_eq!(eval_int_text("16 >> 2"), Some(4));
        assert_eq!(eval_int_text("missing"), None);
    }

    #[test]
    fn reports_text_integer_const_eval_errors() {
        assert!(
            eval_int_text_result("170141183460469231731687303715884105728")
                .unwrap_err()
                .message
                .contains("out of range")
        );
        assert!(
            eval_int_text_result("1 / 0")
                .unwrap_err()
                .message
                .contains("division by zero")
        );
        assert!(
            eval_int_text_result("1 % 0")
                .unwrap_err()
                .message
                .contains("remainder by zero")
        );
        assert!(
            eval_int_text_result("170141183460469231731687303715884105727 + 1")
                .unwrap_err()
                .message
                .contains("overflow")
        );
        assert!(
            eval_int_text_result("1 << 128")
                .unwrap_err()
                .message
                .contains("shift count is out of range")
        );
        assert!(
            eval_array_len_text("-1")
                .unwrap_err()
                .message
                .contains("non-negative")
        );
    }
}
