// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{
    ArrayElements, Block, ComptimeIfExpr, Expr, ExprKind, FieldInit, IndexArg, Item, ItemKind,
    Module, SliceRange, Stmt, StmtKind, SwitchArmBody, SwitchPattern,
};
use nia_comptime_ir::{
    EarlyComptimeAssignTarget, EarlyComptimeBinding, EarlyComptimeExpr, EarlyComptimeExprKind,
    EarlyComptimeName, EarlyComptimeParam, EarlyComptimeTypeArg,
};
use nia_diagnostic::Diagnostic;
use nia_item_tree::ActiveModuleItemTree;
use nia_item_tree::{ComptimeBranch, ComptimeBranchResolver, ItemTreeError, ModuleItemTree};
use nia_node_id::NodeKey;
use nia_span::Span;
use std::collections::{BTreeMap, HashMap, HashSet};

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
    pub active_item_tree: ActiveModuleItemTree,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn prune_module_for_target(module: Module, config: &TargetConfig) -> PruneResult {
    let EarlyComptimeFunctions {
        functions,
        diagnostics,
    } = lower_early_comptime_functions(&module);
    let mut pruner = Pruner {
        config,
        functions,
        diagnostics,
    };
    let pruned = pruner.prune_module(module);
    PruneResult {
        module: pruned.module,
        active_item_tree: pruned.active_item_tree,
        diagnostics: pruner.diagnostics,
    }
}

pub fn eval_config_bool(
    expr: &Expr,
    config: &TargetConfig,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<bool> {
    let functions = HashMap::new();
    let mut env = TargetComptimeEnv {
        config,
        functions: &functions,
        call_locals: Vec::new(),
    };
    let expr = match nia_comptime_ir::lower_expr_early(expr) {
        Ok(expr) => expr,
        Err(err) => {
            diagnostics.push(Diagnostic::user_error_at("E0103", err.span, err.message));
            return None;
        }
    };
    nia_comptime_engine::eval_early_comptime_bool_expr(&expr, &mut env)
        .map_err(|err| diagnostics.push(Diagnostic::user_error_at("E0103", err.span, err.message)))
        .ok()
}

struct TargetComptimeEnv<'a> {
    config: &'a TargetConfig,
    functions: &'a HashMap<String, nia_comptime_ir::EarlyComptimeFunction>,
    call_locals: Vec<TargetComptimeFrame>,
}

#[derive(Debug, Clone, Default)]
struct TargetComptimeFrame {
    values: HashMap<String, nia_comptime_engine::ComptimeValue>,
    mutable_names: HashSet<String>,
}

impl nia_comptime_engine::ComptimeCommonEnv for TargetComptimeEnv<'_> {
    fn resolve_builtin_value(
        &mut self,
        span: Span,
        builtin: nia_ids::ValueBuiltin,
    ) -> Result<nia_comptime_engine::ComptimeValue, nia_comptime_engine::ComptimeError> {
        let _ = span;
        match builtin {
            nia_ids::ValueBuiltin::Builtin => Ok(builtin_comptime_value(self.config)),
        }
    }

    fn push_comptime_scope(
        &mut self,
        _span: Span,
    ) -> Result<(), nia_comptime_engine::ComptimeError> {
        self.call_locals.push(TargetComptimeFrame::default());
        Ok(())
    }

    fn pop_comptime_scope(&mut self) {
        self.call_locals.pop();
    }
}

impl nia_comptime_engine::EarlyComptimeEnv for TargetComptimeEnv<'_> {
    fn resolve_name(
        &mut self,
        span: Span,
        name: &EarlyComptimeName,
    ) -> Result<nia_comptime_engine::ComptimeValue, nia_comptime_engine::ComptimeError> {
        let EarlyComptimeName::Unresolved(name) = name else {
            return Err(nia_comptime_engine::ComptimeError {
                span,
                message: format!(
                    "resolved comptime value `{}` is not available in target conditions",
                    name.display()
                ),
            });
        };
        if let Some(value) = self
            .call_locals
            .iter()
            .rev()
            .find_map(|frame| frame.values.get(name).cloned())
        {
            return Ok(value);
        }
        Err(nia_comptime_engine::ComptimeError {
            span,
            message: format!("unknown target comptime value `{name}`"),
        })
    }

    fn resolve_layout_builtin(
        &mut self,
        span: Span,
        _builtin: nia_ids::LayoutBuiltin,
        _type_arg: &EarlyComptimeTypeArg,
    ) -> Result<nia_comptime_engine::ComptimeValue, nia_comptime_engine::ComptimeError> {
        Err(nia_comptime_engine::ComptimeError {
            span,
            message: "layout builtins are not available in target conditions".to_string(),
        })
    }

    fn call_function(
        &mut self,
        span: Span,
        callee: &EarlyComptimeExpr,
        type_args: &[EarlyComptimeTypeArg],
        arg_exprs: &[EarlyComptimeExpr],
        args: Vec<nia_comptime_engine::ComptimeValue>,
    ) -> Result<nia_comptime_engine::ComptimeValue, nia_comptime_engine::ComptimeError> {
        if !type_args.is_empty() {
            return Err(nia_comptime_engine::ComptimeError {
                span,
                message: "target conditions cannot call generic `comptime fn` before type lowering"
                    .to_string(),
            });
        }
        let _ = arg_exprs;
        let EarlyComptimeExprKind::Ident(name) = &callee.kind else {
            return Err(nia_comptime_engine::ComptimeError {
                span,
                message: "target condition can only call same-module `comptime fn`".to_string(),
            });
        };
        let EarlyComptimeName::Unresolved(name) = name else {
            return Err(nia_comptime_engine::ComptimeError {
                span,
                message: "target condition cannot call resolved semantic comptime functions"
                    .to_string(),
            });
        };
        let Some(function) = self.functions.get(name) else {
            return Err(nia_comptime_engine::ComptimeError {
                span,
                message: format!("unknown target comptime function `{name}`"),
            });
        };
        nia_comptime_engine::eval_early_comptime_function_call(
            span,
            nia_ids::ModuleId(0),
            function,
            Vec::new(),
            args,
            self,
        )
    }

    fn bind_function_param(
        &mut self,
        span: Span,
        param: &EarlyComptimeParam,
        value: nia_comptime_engine::ComptimeValue,
    ) -> Result<(), nia_comptime_engine::ComptimeError> {
        self.bind_named_value(span, &param.name, false, value)
    }

    fn bind_function_local(
        &mut self,
        span: Span,
        binding: &EarlyComptimeBinding,
        value: nia_comptime_engine::ComptimeValue,
    ) -> Result<(), nia_comptime_engine::ComptimeError> {
        self.bind_named_value(span, &binding.name, binding.is_mutable, value)
    }

    fn bind_pattern_local(
        &mut self,
        span: Span,
        name: &str,
        _local_id: Option<nia_ids::LocalId>,
        value: nia_comptime_engine::ComptimeValue,
    ) -> Result<(), nia_comptime_engine::ComptimeError> {
        self.bind_named_value(span, name, false, value)
    }

    fn assign_local(
        &mut self,
        span: Span,
        target: &EarlyComptimeAssignTarget,
        value: nia_comptime_engine::ComptimeValue,
    ) -> Result<(), nia_comptime_engine::ComptimeError> {
        match target {
            EarlyComptimeAssignTarget::Local { name, .. } => {
                self.assign_named_value(span, name, value)
            }
        }
    }
}

impl TargetComptimeEnv<'_> {
    fn bind_named_value(
        &mut self,
        span: Span,
        name: &str,
        is_mutable: bool,
        value: nia_comptime_engine::ComptimeValue,
    ) -> Result<(), nia_comptime_engine::ComptimeError> {
        let Some(frame) = self.call_locals.last_mut() else {
            return Err(nia_comptime_engine::ComptimeError {
                span,
                message: "internal comptime function frame is missing".to_string(),
            });
        };
        if is_mutable {
            frame.mutable_names.insert(name.to_string());
        }
        frame.values.insert(name.to_string(), value);
        Ok(())
    }

    fn assign_named_value(
        &mut self,
        span: Span,
        name: &str,
        value: nia_comptime_engine::ComptimeValue,
    ) -> Result<(), nia_comptime_engine::ComptimeError> {
        for frame in self.call_locals.iter_mut().rev() {
            if frame.values.contains_key(name) {
                if !frame.mutable_names.contains(name) {
                    return Err(nia_comptime_engine::ComptimeError {
                        span,
                        message: format!("cannot assign to immutable comptime local `{name}`"),
                    });
                }
                frame.values.insert(name.to_string(), value);
                return Ok(());
            }
        }
        Err(nia_comptime_engine::ComptimeError {
            span,
            message: format!("unknown comptime assignment target `{name}`"),
        })
    }
}

pub fn builtin_comptime_value(config: &TargetConfig) -> nia_comptime_engine::ComptimeValue {
    let mut fields = BTreeMap::new();
    fields.insert("target".to_string(), target_comptime_value(config));
    nia_comptime_engine::ComptimeValue::Struct(fields)
}

pub fn target_comptime_value(config: &TargetConfig) -> nia_comptime_engine::ComptimeValue {
    let mut fields = BTreeMap::new();
    fields.insert(
        "arch".to_string(),
        nia_comptime_engine::ComptimeValue::String(config.arch.clone()),
    );
    fields.insert(
        "vendor".to_string(),
        nia_comptime_engine::ComptimeValue::String(config.vendor.clone()),
    );
    fields.insert(
        "os".to_string(),
        nia_comptime_engine::ComptimeValue::String(config.os.clone()),
    );
    fields.insert(
        "env".to_string(),
        nia_comptime_engine::ComptimeValue::String(config.env.clone()),
    );
    fields.insert(
        "abi".to_string(),
        nia_comptime_engine::ComptimeValue::String(config.abi.clone()),
    );
    fields.insert(
        "endian".to_string(),
        nia_comptime_engine::ComptimeValue::String(config.endian.clone()),
    );
    fields.insert(
        "pointer_width".to_string(),
        nia_comptime_engine::ComptimeValue::Int(i128::from(config.pointer_width)),
    );
    nia_comptime_engine::ComptimeValue::Struct(fields)
}

struct Pruner<'a> {
    config: &'a TargetConfig,
    functions: HashMap<String, nia_comptime_ir::EarlyComptimeFunction>,
    diagnostics: Vec<Diagnostic>,
}

struct PrunedModule {
    module: Module,
    active_item_tree: ActiveModuleItemTree,
}

impl Pruner<'_> {
    fn prune_module(&mut self, module: Module) -> PrunedModule {
        let tree = ModuleItemTree::from_module(&module);
        let active_item_tree = match tree.active_items(self) {
            Ok(active) => active,
            Err(err) => {
                self.diagnostics
                    .push(Diagnostic::user_error_at("E0103", err.span, err.message));
                ActiveModuleItemTree::new(Vec::new(), Default::default())
            }
        };
        let module = Module {
            items: active_item_tree
                .items
                .iter()
                .map(|item| item.to_ast_item())
                .flat_map(|item| self.prune_item(item))
                .collect(),
        };
        PrunedModule {
            module,
            active_item_tree,
        }
    }

    fn prune_item(&mut self, item: Item) -> Vec<Item> {
        match item.kind {
            ItemKind::ComptimeIf(_) => Vec::new(),
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
                kind: StmtKind::Expr(self.prune_expr(expr)),
                ..stmt
            }],
            StmtKind::Return(value) => vec![Stmt {
                kind: StmtKind::Return(value.map(|value| self.prune_expr(value))),
                ..stmt
            }],
            StmtKind::Defer(expr) => vec![Stmt {
                kind: StmtKind::Defer(self.prune_expr(expr)),
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
            ExprKind::ComptimeIf(comptime_if) => {
                self.prune_comptime_if_expr(span, node_key, *comptime_if)
            }
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
                        *pattern = match std::mem::replace(pattern, SwitchPattern::Default) {
                            SwitchPattern::Default => SwitchPattern::Default,
                            SwitchPattern::OptionalSome {
                                name,
                                span,
                                node_key,
                            } => SwitchPattern::OptionalSome {
                                name,
                                span,
                                node_key,
                            },
                            SwitchPattern::OptionalNull { span } => {
                                SwitchPattern::OptionalNull { span }
                            }
                            SwitchPattern::ErrorOk {
                                name,
                                span,
                                node_key,
                            } => SwitchPattern::ErrorOk {
                                name,
                                span,
                                node_key,
                            },
                            SwitchPattern::ErrorErr {
                                name,
                                span,
                                node_key,
                            } => SwitchPattern::ErrorErr {
                                name,
                                span,
                                node_key,
                            },
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

    fn prune_comptime_if_expr(
        &mut self,
        span: Span,
        node_key: NodeKey,
        comptime_if: ComptimeIfExpr,
    ) -> Expr {
        match self.eval_bool(&comptime_if.cond) {
            Some(true) => Expr {
                span,
                node_key,
                kind: ExprKind::Block(self.prune_block(comptime_if.then_branch)),
            },
            Some(false) => comptime_if.else_branch.map_or(
                Expr {
                    span,
                    node_key: node_key.clone(),
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
                node_key,
                kind: ExprKind::Error,
            },
        }
    }

    fn eval_bool(&mut self, expr: &Expr) -> Option<bool> {
        let mut env = TargetComptimeEnv {
            config: self.config,
            functions: &self.functions,
            call_locals: Vec::new(),
        };
        let expr = match nia_comptime_ir::lower_expr_early(expr) {
            Ok(expr) => expr,
            Err(err) => {
                self.diagnostics
                    .push(Diagnostic::user_error_at("E0103", err.span, err.message));
                return None;
            }
        };
        nia_comptime_engine::eval_early_comptime_bool_expr(&expr, &mut env)
            .map_err(|err| {
                self.diagnostics
                    .push(Diagnostic::user_error_at("E0103", err.span, err.message))
            })
            .ok()
    }
}

impl ComptimeBranchResolver for Pruner<'_> {
    fn resolve_comptime_if(
        &mut self,
        span: Span,
        cond: &Expr,
    ) -> Result<ComptimeBranch, ItemTreeError> {
        let _ = span;
        Ok(match self.eval_bool(cond) {
            Some(true) => ComptimeBranch::Then,
            Some(false) => ComptimeBranch::Else,
            None => ComptimeBranch::None,
        })
    }
}

struct EarlyComptimeFunctions {
    functions: HashMap<String, nia_comptime_ir::EarlyComptimeFunction>,
    diagnostics: Vec<Diagnostic>,
}

fn lower_early_comptime_functions(module: &Module) -> EarlyComptimeFunctions {
    let mut functions = HashMap::new();
    let mut diagnostics = Vec::new();
    for item in &module.items {
        let ItemKind::Function(function) = &item.kind else {
            continue;
        };
        if !function.is_comptime {
            continue;
        }
        match nia_comptime_ir::lower_function_early(function.span, function) {
            Ok(lowered) => {
                functions.insert(function.name.clone(), lowered);
            }
            Err(err) => diagnostics.push(Diagnostic::user_error_at("E0103", err.span, err.message)),
        }
    }
    EarlyComptimeFunctions {
        functions,
        diagnostics,
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
    use nia_ast::ItemKind;
    use nia_parser::parse_module;

    #[test]
    fn prunes_item_comptime_if_with_builtin_target_fields() {
        let (module, errors) = parse_module(
            r#"
comptime if @builtin().target.os == "linux" and @builtin().target.pointer_width == 64 {
    fn selected() i32 { 1 }
} else {
    fn skipped() i32 { 0 }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let result = prune_module_for_target(
            module,
            &TargetConfig {
                arch: "x86_64".to_string(),
                vendor: "unknown".to_string(),
                os: "linux".to_string(),
                env: String::new(),
                abi: String::new(),
                endian: "little".to_string(),
                pointer_width: 64,
            },
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.module.items.len(), 1);
        let ItemKind::Function(function) = &result.module.items[0].kind else {
            panic!("expected selected function");
        };
        assert_eq!(function.name, "selected");
    }

    #[test]
    fn target_condition_cannot_call_runtime_function() {
        let (module, errors) = parse_module(
            r#"
fn enabled() bool { true }

comptime if enabled() {
    fn selected() i32 { 1 }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");

        let result = prune_module_for_target(
            module,
            &TargetConfig {
                arch: "x86_64".to_string(),
                vendor: "unknown".to_string(),
                os: "linux".to_string(),
                env: String::new(),
                abi: String::new(),
                endian: "little".to_string(),
                pointer_width: 64,
            },
        );

        assert!(
            result.diagnostics.iter().any(|diagnostic| diagnostic
                .summary
                .contains("unknown target comptime function `enabled`")),
            "{:?}",
            result.diagnostics
        );
    }

    #[test]
    fn target_condition_rejects_generic_comptime_function_calls() {
        let (module, errors) = parse_module(
            r#"
comptime fn enabled[T]() bool { true }

comptime if enabled[bool]() {
    fn selected() i32 { 1 }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");

        let result = prune_module_for_target(
            module,
            &TargetConfig {
                arch: "x86_64".to_string(),
                vendor: "unknown".to_string(),
                os: "linux".to_string(),
                env: String::new(),
                abi: String::new(),
                endian: "little".to_string(),
                pointer_width: 64,
            },
        );

        assert!(
            result.diagnostics.iter().any(|diagnostic| diagnostic
                .summary
                .contains("target conditions cannot call generic `comptime fn`")),
            "{:?}",
            result.diagnostics
        );
    }
}
