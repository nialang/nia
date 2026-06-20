// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{
    ArrayElements, Attribute, AttributeKind, Block, ConditionBinaryOp, ConditionExpr,
    ConditionExprKind, ConditionUnaryOp, Expr, ExprKind, FieldInit, IndexArg, Item, ItemKind,
    Module, Pattern, PatternKind, SliceRange, Stmt, StmtKind, SwitchArmBody, SwitchPattern,
    SwitchPatternKind,
};
use nia_diagnostic::{Diagnostic, codes};
use nia_item_tree::{ActiveModuleItemTree, ConditionResolver, ItemTreeError, ModuleItemTree};
use nia_span::Span;
use std::collections::HashMap;

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
    pub active_item_tree: ActiveModuleItemTree,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn prune_module_for_target(module: Module, config: &TargetConfig) -> PruneResult {
    let mut pruner = Pruner {
        config,
        diagnostics: Vec::new(),
    };
    let pruned = pruner.prune_module(module);
    PruneResult {
        active_item_tree: pruned.active_item_tree,
        diagnostics: pruner.diagnostics,
    }
}

pub fn eval_config_bool(
    expr: &ConditionExpr,
    config: &TargetConfig,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<bool> {
    match ConditionEvaluator::new(config).eval_bool(expr) {
        Ok(value) => Some(value),
        Err(err) => {
            diagnostics.push(Diagnostic::user_error_at(
                codes::TARGET_CONFIG,
                err.span,
                err.message,
            ));
            None
        }
    }
}

struct Pruner<'a> {
    config: &'a TargetConfig,
    diagnostics: Vec<Diagnostic>,
}

struct PrunedModule {
    active_item_tree: ActiveModuleItemTree,
}

impl Pruner<'_> {
    fn prune_module(&mut self, module: Module) -> PrunedModule {
        let tree = ModuleItemTree::from_module(&module);
        let active_item_tree = match tree.active_items(self) {
            Ok(active) => active,
            Err(err) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TARGET_CONFIG,
                    err.span,
                    err.message,
                ));
                ActiveModuleItemTree::new(Vec::new(), Default::default())
            }
        };
        let inactive_spans = active_item_tree.inactive_spans.clone();
        let module = Module {
            items: active_item_tree
                .items
                .iter()
                .map(|item| item.to_ast_item())
                .flat_map(|item| self.prune_item(item))
                .collect(),
        };
        let item_tree = ModuleItemTree::from_module(&module);
        PrunedModule {
            active_item_tree: ActiveModuleItemTree::new(item_tree.items, inactive_spans),
        }
    }

    fn prune_item(&mut self, item: Item) -> Vec<Item> {
        if !self.attributes_active(&item.attributes, item.span) {
            return Vec::new();
        }
        match item.kind {
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
                for associated_value in &mut extend.associated_values {
                    associated_value.binding.value = associated_value
                        .binding
                        .value
                        .take()
                        .map(|value| self.prune_expr(value));
                }
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
        if !self.attributes_active(&stmt.attributes, stmt.span) {
            return Vec::new();
        }
        match stmt.kind {
            StmtKind::Binding(mut binding) => {
                binding.value = binding.value.map(|value| self.prune_expr(value));
                vec![Stmt {
                    kind: StmtKind::Binding(binding),
                    ..stmt
                }]
            }
            StmtKind::Expr(expr) => vec![Stmt {
                kind: StmtKind::Expr(Box::new(self.prune_expr(*expr))),
                ..stmt
            }],
            StmtKind::Return(value) => vec![Stmt {
                kind: StmtKind::Return(value.map(|value| Box::new(self.prune_expr(*value)))),
                ..stmt
            }],
            StmtKind::Defer(expr) => vec![Stmt {
                kind: StmtKind::Defer(Box::new(self.prune_expr(*expr))),
                ..stmt
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
        let span = expr.span;
        let node_key = expr.node_key.clone();
        match expr.kind {
            ExprKind::Block(block) => Expr {
                span,
                node_key,
                kind: ExprKind::Block(self.prune_block(block)),
            },
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => Expr {
                span,
                node_key,
                kind: ExprKind::If {
                    cond: Box::new(self.prune_expr(*cond)),
                    then_branch: self.prune_block(then_branch),
                    else_branch: else_branch
                        .map(|else_branch| Box::new(self.prune_expr(*else_branch))),
                },
            },
            ExprKind::IfPattern(mut if_pattern) => {
                if_pattern.target = self.prune_expr(if_pattern.target);
                for arm in &mut if_pattern.arms {
                    arm.pattern = self.prune_pattern(std::mem::replace(
                        &mut arm.pattern,
                        Pattern {
                            span: arm.span,
                            kind: PatternKind::Wildcard,
                        },
                    ));
                    arm.body = self.prune_block(std::mem::replace(
                        &mut arm.body,
                        Block {
                            span: arm.span,
                            stmts: Vec::new(),
                            tail: None,
                        },
                    ));
                }
                if_pattern.else_branch = if_pattern
                    .else_branch
                    .map(|else_branch| Box::new(self.prune_expr(*else_branch)));
                Expr {
                    span,
                    node_key,
                    kind: ExprKind::IfPattern(if_pattern),
                }
            }
            ExprKind::Unary { op, expr: inner } => Expr {
                span,
                node_key,
                kind: ExprKind::Unary {
                    op,
                    expr: Box::new(self.prune_expr(*inner)),
                },
            },
            ExprKind::Binary { lhs, op, rhs } => Expr {
                span,
                node_key,
                kind: ExprKind::Binary {
                    lhs: Box::new(self.prune_expr(*lhs)),
                    op,
                    rhs: Box::new(self.prune_expr(*rhs)),
                },
            },
            ExprKind::Assign { lhs, op, rhs } => Expr {
                span,
                node_key,
                kind: ExprKind::Assign {
                    lhs: Box::new(self.prune_expr(*lhs)),
                    op,
                    rhs: Box::new(self.prune_expr(*rhs)),
                },
            },
            ExprKind::Cast { expr: inner, ty } => Expr {
                span,
                node_key,
                kind: ExprKind::Cast {
                    expr: Box::new(self.prune_expr(*inner)),
                    ty,
                },
            },
            ExprKind::Call { callee, args } => Expr {
                span,
                node_key,
                kind: ExprKind::Call {
                    callee: Box::new(self.prune_expr(*callee)),
                    args: args.into_iter().map(|arg| self.prune_expr(arg)).collect(),
                },
            },
            ExprKind::BracketSuffix { callee, args } => Expr {
                span,
                node_key,
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
                span,
                node_key,
                kind: ExprKind::ArrayLiteral {
                    elems: self.prune_array_elements(elems),
                },
            },
            ExprKind::StructLiteral { fields } => Expr {
                span,
                node_key,
                kind: ExprKind::StructLiteral {
                    fields: self.prune_fields(fields),
                },
            },
            ExprKind::TypedArrayLiteral { ty, elems } => Expr {
                span,
                node_key,
                kind: ExprKind::TypedArrayLiteral {
                    ty,
                    elems: self.prune_array_elements(elems),
                },
            },
            ExprKind::TypedStructLiteral { ty, fields } => Expr {
                span,
                node_key,
                kind: ExprKind::TypedStructLiteral {
                    ty,
                    fields: self.prune_fields(fields),
                },
            },
            ExprKind::Qualified { lhs, name } => Expr {
                span,
                node_key,
                kind: ExprKind::Qualified {
                    lhs: Box::new(self.prune_expr(*lhs)),
                    name,
                },
            },
            ExprKind::Field { lhs, name } => Expr {
                span,
                node_key,
                kind: ExprKind::Field {
                    lhs: Box::new(self.prune_expr(*lhs)),
                    name,
                },
            },
            ExprKind::Index { lhs, index } => Expr {
                span,
                node_key,
                kind: ExprKind::Index {
                    lhs: Box::new(self.prune_expr(*lhs)),
                    index: self.prune_index_arg(index),
                },
            },
            ExprKind::Range(range) => Expr {
                span,
                node_key,
                kind: ExprKind::Range(self.prune_range(range)),
            },
            ExprKind::Switch(mut switch) => {
                switch.target = self.prune_expr(switch.target);
                for arm in &mut switch.arms {
                    for pattern in &mut arm.patterns {
                        *pattern = self.prune_switch_pattern(std::mem::replace(
                            pattern,
                            SwitchPattern {
                                span: arm.span,
                                kind: SwitchPatternKind::Wildcard,
                            },
                        ));
                    }
                    arm.body = match std::mem::replace(
                        &mut arm.body,
                        SwitchArmBody::Block(Box::new(Block {
                            span: arm.span,
                            stmts: Vec::new(),
                            tail: None,
                        })),
                    ) {
                        SwitchArmBody::Expr(expr) => {
                            SwitchArmBody::Expr(Box::new(self.prune_expr(*expr)))
                        }
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
                    span,
                    node_key,
                    kind: ExprKind::Switch(switch),
                }
            }
            other => Expr {
                span,
                node_key,
                kind: other,
            },
        }
    }

    fn attributes_active(&mut self, attributes: &[Attribute], owner_span: Span) -> bool {
        for attribute in attributes {
            let AttributeKind::If(cond) = &attribute.kind else {
                continue;
            };
            match ConditionEvaluator::new(self.config).eval_bool(cond) {
                Ok(true) => {}
                Ok(false) => return false,
                Err(err) => {
                    let _ = owner_span;
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TARGET_CONFIG,
                        err.span,
                        err.message,
                    ));
                    return false;
                }
            }
        }
        true
    }

    fn prune_switch_pattern(&mut self, pattern: SwitchPattern) -> SwitchPattern {
        SwitchPattern {
            span: pattern.span,
            kind: match pattern.kind {
                SwitchPatternKind::Wildcard => SwitchPatternKind::Wildcard,
                SwitchPatternKind::Expr(expr) => {
                    SwitchPatternKind::Expr(Box::new(self.prune_expr(*expr)))
                }
                SwitchPatternKind::Range {
                    start,
                    end,
                    inclusive,
                } => SwitchPatternKind::Range {
                    start: Box::new(self.prune_expr(*start)),
                    end: Box::new(self.prune_expr(*end)),
                    inclusive,
                },
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

    fn prune_pattern(&mut self, pattern: Pattern) -> Pattern {
        Pattern {
            span: pattern.span,
            kind: match pattern.kind {
                kind @ (PatternKind::Wildcard
                | PatternKind::Bind { .. }
                | PatternKind::OptionalNull) => kind,
                PatternKind::OptionalSome(pattern) => {
                    PatternKind::OptionalSome(Box::new(self.prune_pattern(*pattern)))
                }
                PatternKind::ErrorOk(pattern) => {
                    PatternKind::ErrorOk(Box::new(self.prune_pattern(*pattern)))
                }
                PatternKind::ErrorErr(pattern) => {
                    PatternKind::ErrorErr(Box::new(self.prune_pattern(*pattern)))
                }
                PatternKind::Expr(expr) => PatternKind::Expr(Box::new(self.prune_expr(*expr))),
                PatternKind::Range {
                    start,
                    end,
                    inclusive,
                } => PatternKind::Range {
                    start: Box::new(self.prune_expr(*start)),
                    end: Box::new(self.prune_expr(*end)),
                    inclusive,
                },
            },
        }
    }

    fn prune_range(&mut self, range: SliceRange) -> SliceRange {
        SliceRange {
            start: range.start.map(|start| Box::new(self.prune_expr(*start))),
            end: range.end.map(|end| Box::new(self.prune_expr(*end))),
            inclusive: range.inclusive,
        }
    }
}

impl ConditionResolver for Pruner<'_> {
    fn resolve_condition(&mut self, cond: &ConditionExpr) -> Result<bool, ItemTreeError> {
        ConditionEvaluator::new(self.config)
            .eval_bool(cond)
            .map_err(|err| ItemTreeError {
                span: err.span,
                message: err.message,
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConditionError {
    span: Span,
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConditionValue {
    Bool(bool),
    Int(i128),
    String(String),
}

struct ConditionEvaluator {
    values: HashMap<&'static str, ConditionValue>,
}

impl ConditionEvaluator {
    fn new(config: &TargetConfig) -> Self {
        let mut values = HashMap::new();
        values.insert("arch", ConditionValue::String(config.arch.clone()));
        values.insert("vendor", ConditionValue::String(config.vendor.clone()));
        values.insert("os", ConditionValue::String(config.os.clone()));
        values.insert("env", ConditionValue::String(config.env.clone()));
        values.insert("abi", ConditionValue::String(config.abi.clone()));
        values.insert("endian", ConditionValue::String(config.endian.clone()));
        values.insert(
            "pointer_width",
            ConditionValue::Int(i128::from(config.pointer_width)),
        );
        Self { values }
    }

    fn eval_bool(&self, expr: &ConditionExpr) -> Result<bool, ConditionError> {
        let value = self.eval(expr)?;
        match value {
            ConditionValue::Bool(value) => Ok(value),
            _ => Err(ConditionError {
                span: expr.span,
                message: "condition expression must evaluate to bool".to_string(),
            }),
        }
    }

    fn eval(&self, expr: &ConditionExpr) -> Result<ConditionValue, ConditionError> {
        match &expr.kind {
            ConditionExprKind::Bool(value) => Ok(ConditionValue::Bool(*value)),
            ConditionExprKind::Integer(text) => {
                let value = nia_comptime_engine::eval_int_literal(text).map_err(|message| {
                    ConditionError {
                        span: expr.span,
                        message,
                    }
                })?;
                Ok(ConditionValue::Int(value))
            }
            ConditionExprKind::String(text) => {
                let literal = nia_comptime_ir::ComptimeStringLiteral {
                    parts: vec![text.clone()],
                };
                let Some(value) = nia_comptime_engine::eval_string_literal(&literal) else {
                    return Err(ConditionError {
                        span: expr.span,
                        message: "invalid condition string literal".to_string(),
                    });
                };
                Ok(ConditionValue::String(value))
            }
            ConditionExprKind::Ident(name) => {
                self.values
                    .get(name.as_str())
                    .cloned()
                    .ok_or_else(|| ConditionError {
                        span: expr.span,
                        message: format!("unknown condition name `{name}`"),
                    })
            }
            ConditionExprKind::Unary { op, expr: inner } => match op {
                ConditionUnaryOp::Not => Ok(ConditionValue::Bool(!self.eval_bool(inner)?)),
            },
            ConditionExprKind::Binary { lhs, op, rhs } => match op {
                ConditionBinaryOp::Eq => {
                    Ok(ConditionValue::Bool(self.eval(lhs)? == self.eval(rhs)?))
                }
                ConditionBinaryOp::Ne => {
                    Ok(ConditionValue::Bool(self.eval(lhs)? != self.eval(rhs)?))
                }
                ConditionBinaryOp::And => {
                    let lhs = self.eval_bool(lhs)?;
                    if !lhs {
                        return Ok(ConditionValue::Bool(false));
                    }
                    Ok(ConditionValue::Bool(self.eval_bool(rhs)?))
                }
                ConditionBinaryOp::Or => {
                    let lhs = self.eval_bool(lhs)?;
                    if lhs {
                        return Ok(ConditionValue::Bool(true));
                    }
                    Ok(ConditionValue::Bool(self.eval_bool(rhs)?))
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use nia_ast::{ItemKind, StmtKind};

    #[test]
    fn conditional_statement_pruning_updates_active_item_tree_bodies() {
        let (module, errors) = nia_parser::parse_module(
            r#"
fn main() i32 {
    @[if false]
    _ = missing_name;
    1
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");

        let pruned = prune_module_for_target(module, &TargetConfig::host());
        assert!(pruned.diagnostics.is_empty(), "{:?}", pruned.diagnostics);

        let active_module = pruned.active_item_tree.to_module();
        let ItemKind::Function(active_function) = &active_module.items[0].kind else {
            panic!("expected function item");
        };
        let active_body = active_function.body.as_ref().expect("expected body");
        assert!(active_body.stmts.is_empty());
    }

    #[test]
    fn selected_conditional_statement_remains_in_active_item_tree_body() {
        let (module, errors) = nia_parser::parse_module(
            r#"
fn main() i32 {
    @[if true]
    _ = 0;
    1
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");

        let pruned = prune_module_for_target(module, &TargetConfig::host());
        assert!(pruned.diagnostics.is_empty(), "{:?}", pruned.diagnostics);

        let active_module = pruned.active_item_tree.to_module();
        let ItemKind::Function(active_function) = &active_module.items[0].kind else {
            panic!("expected function item");
        };
        let active_body = active_function.body.as_ref().expect("expected body");
        assert_eq!(active_body.stmts.len(), 1);
        assert!(matches!(active_body.stmts[0].kind, StmtKind::Expr(_)));
    }
}
