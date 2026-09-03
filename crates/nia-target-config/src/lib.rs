// SPDX-License-Identifier: GPL-3.0-or-later
//! Target-configuration evaluation and conditional module pruning.

use nia_ast::{
    ArrayElements, Attribute, AttributeKind, Block, ConditionBinaryOp, ConditionExpr,
    ConditionExprKind, ConditionUnaryOp, Expr, ExprKind, FieldInit, IndexArg, Item, ItemKind,
    MatchArmBody, Module, Pattern, PatternKind, SliceRange, Stmt, StmtKind,
};
use nia_diagnostic::{Diagnostic, codes};
use nia_item_tree::{ActiveModuleItemTree, ConditionResolver, ItemTreeError, ModuleItemTree};
use nia_span::Span;
use nia_symbol::{SymbolMap, SymbolText, known, symbol_text_from_optional_resolver};

/// Target identity values exposed to conditional compilation expressions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetConfig {
    /// Target architecture name.
    pub arch: String,
    /// Target vendor name.
    pub vendor: String,
    /// Target operating-system name.
    pub os: String,
    /// Target environment name.
    pub env: String,
    /// Target ABI name.
    pub abi: String,
    /// Target byte-order name.
    pub endian: String,
    /// Target pointer width in bits.
    pub pointer_width: u32,
}

impl TargetConfig {
    /// Builds a configuration from the host compilation target.
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

/// Result of pruning target-inactive items and expressions from a module.
#[derive(Debug, Clone, PartialEq)]
pub struct PruneResult {
    /// Active item tree retaining source inactive-span metadata.
    pub active_item_tree: ActiveModuleItemTree,
    /// User-facing diagnostics produced while evaluating target conditions.
    pub diagnostics: Vec<Diagnostic>,
}

/// Prunes a module using the default symbol-resolution behavior.
pub fn prune_module_for_target(module: Module, config: &TargetConfig) -> PruneResult {
    prune_module_for_target_with_symbols(module, config, None)
}

/// Prunes a module and uses an optional symbol provider for diagnostics.
pub fn prune_module_for_target_with_symbols(
    module: Module,
    config: &TargetConfig,
    symbols: Option<&dyn SymbolText>,
) -> PruneResult {
    let mut pruner = Pruner {
        config,
        symbols,
        diagnostics: Vec::new(),
    };
    let pruned = pruner.prune_module(module);
    PruneResult {
        active_item_tree: pruned.active_item_tree,
        diagnostics: pruner.diagnostics,
    }
}

/// Evaluates a target condition and records failures as diagnostics.
pub fn eval_config_bool(
    expr: &ConditionExpr,
    config: &TargetConfig,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<bool> {
    eval_config_bool_with_symbols(expr, config, diagnostics, None)
}

/// Evaluates a target condition with an optional symbol provider for diagnostics.
pub fn eval_config_bool_with_symbols(
    expr: &ConditionExpr,
    config: &TargetConfig,
    diagnostics: &mut Vec<Diagnostic>,
    symbols: Option<&dyn SymbolText>,
) -> Option<bool> {
    match ConditionEvaluator::new(config, symbols).eval_bool(expr) {
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
    symbols: Option<&'a dyn SymbolText>,
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
            StmtKind::Static(mut binding) => {
                binding.value = binding.value.map(|value| self.prune_expr(value));
                vec![Stmt {
                    kind: StmtKind::Static(binding),
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
                let pattern_span = if_pattern.pattern.span;
                if_pattern.pattern = self.prune_pattern(std::mem::replace(
                    &mut if_pattern.pattern,
                    Pattern {
                        span: pattern_span,
                        kind: PatternKind::Wildcard,
                    },
                ));
                let then_span = if_pattern.then_branch.span;
                if_pattern.then_branch = self.prune_block(std::mem::replace(
                    &mut if_pattern.then_branch,
                    Block {
                        span: then_span,
                        stmts: Vec::new(),
                        tail: None,
                    },
                ));
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
            ExprKind::Tuple(elems) => Expr {
                span,
                node_key,
                kind: ExprKind::Tuple(
                    elems
                        .into_iter()
                        .map(|elem| self.prune_expr(elem))
                        .collect(),
                ),
            },
            ExprKind::ArrayLiteral { elems } => Expr {
                span,
                node_key,
                kind: ExprKind::ArrayLiteral {
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
            ExprKind::Match(mut matched) => {
                matched.target = self.prune_expr(matched.target);
                for arm in &mut matched.arms {
                    for pattern in &mut arm.patterns {
                        *pattern = self.prune_pattern(std::mem::replace(
                            pattern,
                            Pattern {
                                span: arm.span,
                                kind: PatternKind::Wildcard,
                            },
                        ));
                    }
                    arm.body = match std::mem::replace(
                        &mut arm.body,
                        MatchArmBody::Block(Box::new(Block {
                            span: arm.span,
                            stmts: Vec::new(),
                            tail: None,
                        })),
                    ) {
                        MatchArmBody::Expr(expr) => {
                            MatchArmBody::Expr(Box::new(self.prune_expr(*expr)))
                        }
                        MatchArmBody::Stmt(stmt) => {
                            let mut stmts = self.prune_stmt(*stmt);
                            if stmts.len() == 1 {
                                MatchArmBody::Stmt(Box::new(stmts.remove(0)))
                            } else {
                                MatchArmBody::Block(Box::new(Block {
                                    span: arm.span,
                                    stmts,
                                    tail: None,
                                }))
                            }
                        }
                        MatchArmBody::Block(block) => {
                            MatchArmBody::Block(Box::new(self.prune_block(*block)))
                        }
                    };
                }
                Expr {
                    span,
                    node_key,
                    kind: ExprKind::Match(matched),
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
            match ConditionEvaluator::new(self.config, self.symbols).eval_bool(cond) {
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
                PatternKind::Pointer(pattern) => {
                    PatternKind::Pointer(Box::new(self.prune_pattern(*pattern)))
                }
                PatternKind::MutPointer(pattern) => {
                    PatternKind::MutPointer(Box::new(self.prune_pattern(*pattern)))
                }
                PatternKind::OptionalSome(pattern) => {
                    PatternKind::OptionalSome(Box::new(self.prune_pattern(*pattern)))
                }
                PatternKind::ErrorOk(pattern) => {
                    PatternKind::ErrorOk(Box::new(self.prune_pattern(*pattern)))
                }
                PatternKind::ErrorErr(pattern) => {
                    PatternKind::ErrorErr(Box::new(self.prune_pattern(*pattern)))
                }
                PatternKind::Tuple(fields) => PatternKind::Tuple(
                    fields
                        .into_iter()
                        .map(|field| self.prune_pattern(field))
                        .collect(),
                ),
                PatternKind::Nominal {
                    constructor: variant,
                    fields,
                } => PatternKind::Nominal {
                    constructor: Box::new(self.prune_expr(*variant)),
                    fields: match fields {
                        nia_ast::NominalPatternFields::Tuple(fields) => {
                            nia_ast::NominalPatternFields::Tuple(
                                fields
                                    .into_iter()
                                    .map(|field| self.prune_pattern(field))
                                    .collect(),
                            )
                        }
                        nia_ast::NominalPatternFields::Named { fields, rest } => {
                            nia_ast::NominalPatternFields::Named {
                                fields: fields
                                    .into_iter()
                                    .map(|mut field| {
                                        field.pattern = self.prune_pattern(field.pattern);
                                        field
                                    })
                                    .collect(),
                                rest,
                            }
                        }
                    },
                },
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
        ConditionEvaluator::new(self.config, self.symbols)
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
    Int(u128),
    String(String),
}

struct ConditionEvaluator<'a> {
    values: SymbolMap<ConditionValue>,
    symbols: Option<&'a dyn SymbolText>,
}

impl<'a> ConditionEvaluator<'a> {
    fn new(config: &TargetConfig, symbols: Option<&'a dyn SymbolText>) -> Self {
        let mut values = SymbolMap::default();
        values.insert(known::ARCH, ConditionValue::String(config.arch.clone()));
        values.insert(known::VENDOR, ConditionValue::String(config.vendor.clone()));
        values.insert(known::OS, ConditionValue::String(config.os.clone()));
        values.insert(known::ENV, ConditionValue::String(config.env.clone()));
        values.insert(known::ABI, ConditionValue::String(config.abi.clone()));
        values.insert(known::ENDIAN, ConditionValue::String(config.endian.clone()));
        values.insert(
            known::POINTER_WIDTH,
            ConditionValue::Int(u128::from(config.pointer_width)),
        );
        Self { values, symbols }
    }

    fn symbol_name(&self, symbol: nia_symbol::SymbolId) -> String {
        symbol_text_from_optional_resolver(self.symbols, symbol)
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
                let value =
                    nia_literals::eval_int_literal(text).map_err(|message| ConditionError {
                        span: expr.span,
                        message,
                    })?;
                Ok(ConditionValue::Int(value))
            }
            ConditionExprKind::String(text) => {
                let Some(value) = nia_literals::eval_string_literal_parts([text.as_str()]) else {
                    return Err(ConditionError {
                        span: expr.span,
                        message: "invalid condition string literal".to_string(),
                    });
                };
                Ok(ConditionValue::String(value))
            }
            ConditionExprKind::Ident(name) => {
                self.values
                    .get(name)
                    .cloned()
                    .ok_or_else(|| ConditionError {
                        span: expr.span,
                        message: format!("unknown condition name `{}`", self.symbol_name(*name)),
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

    #[test]
    fn target_identity_selects_items_for_simulated_ilp32_big_endian_target() {
        let (module, errors) = nia_parser::parse_module(
            r#"
@[if arch == "mips" and endian == "big" and pointerWidth == 32]
fn selected() i32 { 1 }

@[if arch == "x86_64" or pointerWidth == 64]
fn rejected() i32 { 2 }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");

        let config = TargetConfig {
            arch: "mips".to_string(),
            vendor: "unknown".to_string(),
            os: "freestanding".to_string(),
            env: String::new(),
            abi: String::new(),
            endian: "big".to_string(),
            pointer_width: 32,
        };
        let pruned = prune_module_for_target(module, &config);
        assert!(pruned.diagnostics.is_empty(), "{:?}", pruned.diagnostics);

        let active_module = pruned.active_item_tree.to_module();
        assert_eq!(active_module.items.len(), 1);
        assert!(matches!(active_module.items[0].kind, ItemKind::Function(_)));
    }

    #[test]
    fn condition_evaluation_short_circuits_unknown_names() {
        let (module, errors) = nia_parser::parse_module(
            r#"
@[if false and missing_target_name]
fn hidden() i32 { 1 }

@[if true or missing_target_name]
fn visible() i32 { 2 }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");

        let pruned = prune_module_for_target(module, &TargetConfig::host());
        assert!(pruned.diagnostics.is_empty(), "{:?}", pruned.diagnostics);
        let active_module = pruned.active_item_tree.to_module();
        assert_eq!(active_module.items.len(), 1);
        assert!(matches!(active_module.items[0].kind, ItemKind::Function(_)));
    }

    #[test]
    fn condition_type_mismatch_is_reported_as_target_diagnostic() {
        let (module, errors) = nia_parser::parse_module(
            r#"
@[if pointerWidth]
fn invalid() i32 { 1 }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");

        let pruned = prune_module_for_target(module, &TargetConfig::host());
        assert_eq!(pruned.active_item_tree.to_module().items.len(), 0);
        assert_eq!(pruned.diagnostics.len(), 1);
        assert!(pruned.diagnostics[0].summary.contains("condition"));
    }
}
