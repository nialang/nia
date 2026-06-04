// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{
    ArrayElements, BinaryOp, Block, ComptimeIfExpr, ComptimeIfItem, ComptimeIfItemElse, Expr,
    ExprKind, FieldInit, IndexArg, Item, ItemKind, Module, SliceRange, Stmt, StmtKind,
    StringLiteral, SwitchArmBody, SwitchPattern, UnaryOp,
};
use nia_diagnostic::Diagnostic;
use nia_span::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetConfig {
    pub arch: String,
    pub vendor: String,
    pub os: String,
    pub env: String,
    pub abi: String,
    pub endian: String,
    pub pointer_width: u32,
}

impl TargetConfig {
    pub fn host() -> Self {
        Self {
            arch: std::env::consts::ARCH.to_string(),
            vendor: "unknown".to_string(),
            os: std::env::consts::OS.to_string(),
            env: String::new(),
            abi: String::new(),
            endian: endian().to_string(),
            pointer_width: usize::BITS,
        }
    }
}

impl Default for TargetConfig {
    fn default() -> Self {
        Self::host()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PruneResult {
    pub module: Module,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn prune_module_for_target(module: Module, config: &TargetConfig) -> PruneResult {
    let mut pruner = Pruner {
        config,
        diagnostics: Vec::new(),
    };
    let module = pruner.prune_module(module);
    PruneResult {
        module,
        diagnostics: pruner.diagnostics,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigValue {
    Bool(bool),
    String(String),
    Int(u64),
}

pub fn eval_config_bool(
    expr: &Expr,
    config: &TargetConfig,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<bool> {
    match eval_config_value(expr, config, diagnostics) {
        Some(ConfigValue::Bool(value)) => Some(value),
        Some(_) => {
            diagnostics.push(Diagnostic::error(
                expr.span,
                "comptime if condition must evaluate to bool",
            ));
            None
        }
        None => None,
    }
}

fn eval_config_value(
    expr: &Expr,
    config: &TargetConfig,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<ConfigValue> {
    match &expr.kind {
        ExprKind::Bool(value) => Some(ConfigValue::Bool(*value)),
        ExprKind::String(literal) => literal_string(literal).map(ConfigValue::String),
        ExprKind::Integer(text) => parse_u64_literal(text).map(ConfigValue::Int).or_else(|| {
            diagnostics.push(Diagnostic::error(
                expr.span,
                "target config integer literal must fit in u64",
            ));
            None
        }),
        ExprKind::Unary {
            op: UnaryOp::Not,
            expr,
        } => eval_config_bool(expr, config, diagnostics).map(|value| ConfigValue::Bool(!value)),
        ExprKind::Binary { lhs, op, rhs } => {
            eval_config_binary(expr.span, lhs, *op, rhs, config, diagnostics)
        }
        ExprKind::Field { lhs, name } => {
            eval_config_field(expr.span, lhs, name, config, diagnostics)
        }
        ExprKind::Call { .. } => {
            diagnostics.push(Diagnostic::error(
                expr.span,
                "target config calls are only supported as `@builtin()` field roots",
            ));
            None
        }
        ExprKind::Block(block) => {
            if !block.stmts.is_empty() {
                diagnostics.push(Diagnostic::error(
                    expr.span,
                    "target config block conditions cannot contain statements",
                ));
                return None;
            }
            block
                .tail
                .as_deref()
                .and_then(|tail| eval_config_value(tail, config, diagnostics))
        }
        _ => {
            diagnostics.push(Diagnostic::error(
                expr.span,
                "unsupported expression in target config condition",
            ));
            None
        }
    }
}

fn eval_config_binary(
    span: Span,
    lhs: &Expr,
    op: BinaryOp,
    rhs: &Expr,
    config: &TargetConfig,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<ConfigValue> {
    match op {
        BinaryOp::And => {
            let lhs = eval_config_bool(lhs, config, diagnostics)?;
            if !lhs {
                return Some(ConfigValue::Bool(false));
            }
            eval_config_bool(rhs, config, diagnostics).map(ConfigValue::Bool)
        }
        BinaryOp::Or => {
            let lhs = eval_config_bool(lhs, config, diagnostics)?;
            if lhs {
                return Some(ConfigValue::Bool(true));
            }
            eval_config_bool(rhs, config, diagnostics).map(ConfigValue::Bool)
        }
        BinaryOp::Eq | BinaryOp::Ne => {
            let lhs = eval_config_value(lhs, config, diagnostics)?;
            let rhs = eval_config_value(rhs, config, diagnostics)?;
            let equal = config_values_equal(&lhs, &rhs).unwrap_or_else(|| {
                diagnostics.push(Diagnostic::error(
                    span,
                    "target config equality requires matching operand types",
                ));
                false
            });
            Some(ConfigValue::Bool(if op == BinaryOp::Eq {
                equal
            } else {
                !equal
            }))
        }
        _ => {
            diagnostics.push(Diagnostic::error(
                span,
                "unsupported operator in target config condition",
            ));
            None
        }
    }
}

fn eval_config_field(
    span: Span,
    lhs: &Expr,
    name: &str,
    config: &TargetConfig,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<ConfigValue> {
    let Some(root) = target_field_root(lhs) else {
        diagnostics.push(Diagnostic::error(
            span,
            "target config fields are only available on `@builtin().target`",
        ));
        return None;
    };
    if root != "target" {
        diagnostics.push(Diagnostic::error(
            span,
            "target config fields are only available on `@builtin().target`",
        ));
        return None;
    }
    match name {
        "arch" => Some(ConfigValue::String(config.arch.clone())),
        "vendor" => Some(ConfigValue::String(config.vendor.clone())),
        "os" => Some(ConfigValue::String(config.os.clone())),
        "env" => Some(ConfigValue::String(config.env.clone())),
        "abi" => Some(ConfigValue::String(config.abi.clone())),
        "endian" => Some(ConfigValue::String(config.endian.clone())),
        "pointer_width" => Some(ConfigValue::Int(config.pointer_width as u64)),
        _ => {
            diagnostics.push(Diagnostic::error(
                span,
                format!("unknown target config field `{name}`"),
            ));
            None
        }
    }
}

fn target_field_root(expr: &Expr) -> Option<&str> {
    let ExprKind::Field { lhs, name } = &expr.kind else {
        return None;
    };
    if name != "target" {
        return None;
    }
    let ExprKind::Call { callee, args } = &lhs.kind else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let ExprKind::Builtin { name, type_arg } = &callee.kind else {
        return None;
    };
    if name == "builtin" && type_arg.is_none() {
        Some("target")
    } else {
        None
    }
}

fn config_values_equal(lhs: &ConfigValue, rhs: &ConfigValue) -> Option<bool> {
    match (lhs, rhs) {
        (ConfigValue::Bool(lhs), ConfigValue::Bool(rhs)) => Some(lhs == rhs),
        (ConfigValue::String(lhs), ConfigValue::String(rhs)) => Some(lhs == rhs),
        (ConfigValue::Int(lhs), ConfigValue::Int(rhs)) => Some(lhs == rhs),
        _ => None,
    }
}

fn literal_string(literal: &StringLiteral) -> Option<String> {
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

fn parse_u64_literal(text: &str) -> Option<u64> {
    let text = text.replace('_', "");
    let digits = text
        .trim_end_matches(|ch: char| ch.is_ascii_alphabetic())
        .trim_end_matches(['8', '6', '3', '2']);
    if let Some(hex) = digits.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).ok()
    } else if let Some(binary) = digits.strip_prefix("0b") {
        u64::from_str_radix(binary, 2).ok()
    } else if let Some(octal) = digits.strip_prefix("0o") {
        u64::from_str_radix(octal, 8).ok()
    } else {
        digits.parse().ok()
    }
}

struct Pruner<'a> {
    config: &'a TargetConfig,
    diagnostics: Vec<Diagnostic>,
}

impl Pruner<'_> {
    fn prune_module(&mut self, module: Module) -> Module {
        Module {
            items: module
                .items
                .into_iter()
                .flat_map(|item| self.prune_item(item))
                .collect(),
        }
    }

    fn prune_item(&mut self, item: Item) -> Vec<Item> {
        match item.kind {
            ItemKind::ComptimeIf(comptime_if) => self.prune_comptime_if_item(comptime_if),
            ItemKind::Struct(item_struct) => {
                vec![Item {
                    kind: ItemKind::Struct(item_struct),
                    ..item
                }]
            }
            ItemKind::Function(mut function) => {
                function.body = function.body.map(|body| self.prune_block(body));
                vec![Item {
                    kind: ItemKind::Function(function),
                    ..item
                }]
            }
            ItemKind::Trait(mut item_trait) => {
                for method in &mut item_trait.methods {
                    method.function.body = method
                        .function
                        .body
                        .take()
                        .map(|body| self.prune_block(body));
                }
                vec![Item {
                    kind: ItemKind::Trait(item_trait),
                    ..item
                }]
            }
            ItemKind::Extend(mut extend) => {
                for method in &mut extend.methods {
                    method.function.body = method
                        .function
                        .body
                        .take()
                        .map(|body| self.prune_block(body));
                }
                vec![Item {
                    kind: ItemKind::Extend(extend),
                    ..item
                }]
            }
            _ => vec![item],
        }
    }

    fn prune_comptime_if_item(&mut self, comptime_if: ComptimeIfItem) -> Vec<Item> {
        match eval_config_bool(&comptime_if.cond, self.config, &mut self.diagnostics) {
            Some(true) => comptime_if
                .then_items
                .into_iter()
                .flat_map(|item| self.prune_item(item))
                .collect(),
            Some(false) => match comptime_if.else_branch {
                Some(ComptimeIfItemElse::If(comptime_if)) => {
                    self.prune_comptime_if_item(*comptime_if)
                }
                Some(ComptimeIfItemElse::Items(items)) => items
                    .into_iter()
                    .flat_map(|item| self.prune_item(item))
                    .collect(),
                None => Vec::new(),
            },
            None => Vec::new(),
        }
    }

    fn prune_block(&mut self, block: Block) -> Block {
        Block {
            span: block.span,
            stmts: block
                .stmts
                .into_iter()
                .flat_map(|stmt| self.prune_stmt(stmt))
                .collect(),
            tail: block.tail.map(|tail| Box::new(self.prune_expr(*tail))),
        }
    }

    fn prune_stmt(&mut self, stmt: Stmt) -> Vec<Stmt> {
        match stmt.kind {
            StmtKind::Binding(mut binding) => {
                binding.value = binding.value.map(|value| self.prune_expr(value));
                vec![Stmt {
                    kind: StmtKind::Binding(binding),
                    ..stmt
                }]
            }
            StmtKind::Expr(expr) => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Expr(self.prune_expr(expr)),
            }],
            StmtKind::Return(value) => vec![Stmt {
                kind: StmtKind::Return(value.map(|value| self.prune_expr(value))),
                ..stmt
            }],
            StmtKind::Defer(expr) => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Defer(self.prune_expr(expr)),
            }],
            StmtKind::ForIn(mut for_stmt) => {
                for_stmt.iter = self.prune_expr(for_stmt.iter);
                for_stmt.body = self.prune_block(for_stmt.body);
                vec![Stmt {
                    kind: StmtKind::ForIn(for_stmt),
                    ..stmt
                }]
            }
            StmtKind::While(mut while_stmt) => {
                while_stmt.cond = self.prune_expr(while_stmt.cond);
                while_stmt.body = self.prune_block(while_stmt.body);
                vec![Stmt {
                    kind: StmtKind::While(while_stmt),
                    ..stmt
                }]
            }
            StmtKind::Loop(mut loop_stmt) => {
                loop_stmt.body = self.prune_block(loop_stmt.body);
                vec![Stmt {
                    kind: StmtKind::Loop(loop_stmt),
                    ..stmt
                }]
            }
            StmtKind::Using(_) | StmtKind::Break | StmtKind::Continue => vec![stmt],
        }
    }

    fn prune_expr(&mut self, expr: Expr) -> Expr {
        match expr.kind {
            ExprKind::ComptimeIf(comptime_if) => {
                self.prune_comptime_if_expr(expr.span, *comptime_if)
            }
            ExprKind::Block(block) => Expr {
                span: expr.span,
                kind: ExprKind::Block(self.prune_block(block)),
            },
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => Expr {
                span: expr.span,
                kind: ExprKind::If {
                    cond: Box::new(self.prune_expr(*cond)),
                    then_branch: self.prune_block(then_branch),
                    else_branch: else_branch
                        .map(|else_branch| Box::new(self.prune_expr(*else_branch))),
                },
            },
            ExprKind::Unary { op, expr: inner } => Expr {
                span: expr.span,
                kind: ExprKind::Unary {
                    op,
                    expr: Box::new(self.prune_expr(*inner)),
                },
            },
            ExprKind::Binary { lhs, op, rhs } => Expr {
                span: expr.span,
                kind: ExprKind::Binary {
                    lhs: Box::new(self.prune_expr(*lhs)),
                    op,
                    rhs: Box::new(self.prune_expr(*rhs)),
                },
            },
            ExprKind::Assign { lhs, op, rhs } => Expr {
                span: expr.span,
                kind: ExprKind::Assign {
                    lhs: Box::new(self.prune_expr(*lhs)),
                    op,
                    rhs: Box::new(self.prune_expr(*rhs)),
                },
            },
            ExprKind::Cast { expr: inner, ty } => Expr {
                span: expr.span,
                kind: ExprKind::Cast {
                    expr: Box::new(self.prune_expr(*inner)),
                    ty,
                },
            },
            ExprKind::Call { callee, args } => Expr {
                span: expr.span,
                kind: ExprKind::Call {
                    callee: Box::new(self.prune_expr(*callee)),
                    args: args.into_iter().map(|arg| self.prune_expr(arg)).collect(),
                },
            },
            ExprKind::BracketSuffix { callee, args } => Expr {
                span: expr.span,
                kind: ExprKind::BracketSuffix {
                    callee: Box::new(self.prune_expr(*callee)),
                    args: args
                        .into_iter()
                        .map(|mut arg| {
                            arg.expr = arg.expr.map(|expr| self.prune_expr(expr));
                            arg
                        })
                        .collect(),
                },
            },
            ExprKind::ArrayLiteral { elems } => Expr {
                span: expr.span,
                kind: ExprKind::ArrayLiteral {
                    elems: self.prune_array_elements(elems),
                },
            },
            ExprKind::StructLiteral { fields } => Expr {
                span: expr.span,
                kind: ExprKind::StructLiteral {
                    fields: self.prune_fields(fields),
                },
            },
            ExprKind::TypedArrayLiteral { ty, elems } => Expr {
                span: expr.span,
                kind: ExprKind::TypedArrayLiteral {
                    ty,
                    elems: self.prune_array_elements(elems),
                },
            },
            ExprKind::TypedStructLiteral { ty, fields } => Expr {
                span: expr.span,
                kind: ExprKind::TypedStructLiteral {
                    ty,
                    fields: self.prune_fields(fields),
                },
            },
            ExprKind::Qualified { lhs, name } => Expr {
                span: expr.span,
                kind: ExprKind::Qualified {
                    lhs: Box::new(self.prune_expr(*lhs)),
                    name,
                },
            },
            ExprKind::Field { lhs, name } => Expr {
                span: expr.span,
                kind: ExprKind::Field {
                    lhs: Box::new(self.prune_expr(*lhs)),
                    name,
                },
            },
            ExprKind::Index { lhs, index } => Expr {
                span: expr.span,
                kind: ExprKind::Index {
                    lhs: Box::new(self.prune_expr(*lhs)),
                    index: self.prune_index_arg(index),
                },
            },
            ExprKind::Range(range) => Expr {
                span: expr.span,
                kind: ExprKind::Range(self.prune_range(range)),
            },
            ExprKind::Switch(mut switch) => {
                switch.target = self.prune_expr(switch.target);
                for arm in &mut switch.arms {
                    for pattern in &mut arm.patterns {
                        *pattern = match std::mem::replace(pattern, SwitchPattern::Default) {
                            SwitchPattern::Default => SwitchPattern::Default,
                            SwitchPattern::OptionalSome { name, span } => {
                                SwitchPattern::OptionalSome { name, span }
                            }
                            SwitchPattern::OptionalNull { span } => {
                                SwitchPattern::OptionalNull { span }
                            }
                            SwitchPattern::ErrorOk { name, span } => {
                                SwitchPattern::ErrorOk { name, span }
                            }
                            SwitchPattern::ErrorErr { name, span } => {
                                SwitchPattern::ErrorErr { name, span }
                            }
                            SwitchPattern::Expr(expr) => SwitchPattern::Expr(self.prune_expr(expr)),
                            SwitchPattern::Range {
                                start,
                                end,
                                inclusive,
                                span,
                            } => SwitchPattern::Range {
                                start: self.prune_expr(start),
                                end: self.prune_expr(end),
                                inclusive,
                                span,
                            },
                        };
                    }
                    arm.body = match std::mem::replace(
                        &mut arm.body,
                        SwitchArmBody::Block(Box::new(Block {
                            span: arm.span,
                            stmts: Vec::new(),
                            tail: None,
                        })),
                    ) {
                        SwitchArmBody::Expr(expr) => SwitchArmBody::Expr(self.prune_expr(expr)),
                        SwitchArmBody::Stmt(stmt) => {
                            let mut stmts = self.prune_stmt(*stmt);
                            if stmts.len() == 1 {
                                SwitchArmBody::Stmt(Box::new(stmts.remove(0)))
                            } else {
                                SwitchArmBody::Block(Box::new(Block {
                                    span: arm.span,
                                    stmts,
                                    tail: None,
                                }))
                            }
                        }
                        SwitchArmBody::Block(block) => {
                            SwitchArmBody::Block(Box::new(self.prune_block(*block)))
                        }
                    };
                }
                Expr {
                    span: expr.span,
                    kind: ExprKind::Switch(switch),
                }
            }
            other => Expr {
                span: expr.span,
                kind: other,
            },
        }
    }

    fn prune_array_elements(&mut self, elems: ArrayElements) -> ArrayElements {
        match elems {
            ArrayElements::List(elems) => ArrayElements::List(
                elems
                    .into_iter()
                    .map(|expr| self.prune_expr(expr))
                    .collect(),
            ),
            ArrayElements::Repeat { value, count } => ArrayElements::Repeat {
                value: Box::new(self.prune_expr(*value)),
                count: Box::new(self.prune_expr(*count)),
            },
        }
    }

    fn prune_fields(&mut self, fields: Vec<FieldInit>) -> Vec<FieldInit> {
        fields
            .into_iter()
            .map(|mut field| {
                field.value = self.prune_expr(field.value);
                field
            })
            .collect()
    }

    fn prune_index_arg(&mut self, index: IndexArg) -> IndexArg {
        match index {
            IndexArg::Expr(expr) => IndexArg::Expr(Box::new(self.prune_expr(*expr))),
            IndexArg::Range(range) => IndexArg::Range(self.prune_range(range)),
        }
    }

    fn prune_range(&mut self, range: SliceRange) -> SliceRange {
        SliceRange {
            start: range.start.map(|start| Box::new(self.prune_expr(*start))),
            end: range.end.map(|end| Box::new(self.prune_expr(*end))),
            inclusive: range.inclusive,
        }
    }

    fn prune_comptime_if_expr(&mut self, span: Span, comptime_if: ComptimeIfExpr) -> Expr {
        match eval_config_bool(&comptime_if.cond, self.config, &mut self.diagnostics) {
            Some(true) => Expr {
                span,
                kind: ExprKind::Block(self.prune_block(comptime_if.then_branch)),
            },
            Some(false) => comptime_if.else_branch.map_or(
                Expr {
                    span,
                    kind: ExprKind::Block(Block {
                        span,
                        stmts: Vec::new(),
                        tail: None,
                    }),
                },
                |else_branch| self.prune_expr(*else_branch),
            ),
            None => Expr {
                span,
                kind: ExprKind::Error,
            },
        }
    }
}

fn endian() -> &'static str {
    if cfg!(target_endian = "little") {
        "little"
    } else {
        "big"
    }
}
