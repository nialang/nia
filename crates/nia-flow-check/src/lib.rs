// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashSet;

use nia_ast::{
    Block, Expr, ExprKind, ForHeader, FunctionItem, IndexArg, ItemKind, Module, Stmt, StmtKind,
    SwitchArmBody, SwitchPattern,
};
use nia_diagnostic::Diagnostic;
use nia_item_signatures::{FunctionSignature, ItemSignatures};
use nia_ty::{PrimitiveTy, TyInterner, TyKind};

#[derive(Debug, Clone, PartialEq)]
pub struct FlowCheck {
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Flow {
    falls_through: bool,
}

pub fn check_module_flow(
    module: &Module,
    interner: &TyInterner,
    signatures: &ItemSignatures,
) -> FlowCheck {
    let mut checker = FlowChecker {
        interner,
        signatures,
        diagnostics: Vec::new(),
        loop_depth: 0,
    };
    checker.check_module(module);
    FlowCheck {
        diagnostics: checker.diagnostics,
    }
}

struct FlowChecker<'a> {
    interner: &'a TyInterner,
    signatures: &'a ItemSignatures,
    diagnostics: Vec<Diagnostic>,
    loop_depth: usize,
}

impl FlowChecker<'_> {
    fn check_module(&mut self, module: &Module) {
        for item in &module.items {
            match &item.kind {
                ItemKind::Function(function) => self.check_function(function),
                ItemKind::Extend(extend) => {
                    for method in &extend.methods {
                        self.check_function(&method.function);
                    }
                }
                ItemKind::Import(_)
                | ItemKind::Using(_)
                | ItemKind::Struct(_)
                | ItemKind::Union(_)
                | ItemKind::Enum(_)
                | ItemKind::TypeAlias(_)
                | ItemKind::Binding(_) => {}
            }
        }
    }

    fn check_function(&mut self, function: &FunctionItem) {
        let Some(body) = &function.body else {
            return;
        };
        let flow = self.check_block(body);
        let tail_returns = body
            .tail
            .as_deref()
            .is_some_and(|tail| self.tail_expr_returns_on_all_paths(tail));
        if self.function_requires_return(function) && flow.falls_through && !tail_returns {
            self.diagnostics.push(Diagnostic::error(
                body.span,
                "non-void function does not return on all reachable paths",
            ));
        }
    }

    fn function_requires_return(&self, function: &FunctionItem) -> bool {
        let Some(signature) = self.signature_for_function(function) else {
            return false;
        };
        !matches!(
            self.interner.get(signature.return_type),
            Some(TyKind::Primitive(PrimitiveTy::Void))
        )
    }

    fn signature_for_function(&self, function: &FunctionItem) -> Option<&FunctionSignature> {
        self.signatures
            .functions
            .values()
            .find(|signature| signature.span == function.span)
    }

    fn check_block(&mut self, block: &Block) -> Flow {
        let mut falls_through = true;
        for stmt in &block.stmts {
            if !falls_through {
                self.diagnostics
                    .push(Diagnostic::error(stmt.span, "unreachable statement"));
                self.check_stmt(stmt);
                continue;
            }
            falls_through = self.check_stmt(stmt).falls_through;
        }
        if falls_through && block.tail.is_some() {
            falls_through = true;
        }
        Flow { falls_through }
    }

    fn tail_expr_returns_on_all_paths(&mut self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                let then_returns = self.block_returns_on_all_paths(then_branch);
                let else_returns = else_branch
                    .as_deref()
                    .is_some_and(|else_branch| self.tail_expr_returns_on_all_paths(else_branch));
                then_returns && else_returns
            }
            ExprKind::Block(block) if block.stmts.is_empty() && block.tail.is_none() => true,
            ExprKind::Block(block) => self.block_returns_on_all_paths(block),
            ExprKind::Switch(switch) => self.switch_tail_covers_all_paths(switch),
            _ => true,
        }
    }

    fn switch_tail_covers_all_paths(&mut self, switch: &nia_ast::SwitchStmt) -> bool {
        let mut seen_default = false;
        let mut seen_patterns = HashSet::new();
        let mut all_arms_produce = !switch.arms.is_empty();
        for arm in &switch.arms {
            match &arm.pattern {
                SwitchPattern::Default => {
                    if seen_default {
                        self.diagnostics
                            .push(Diagnostic::error(arm.span, "duplicate switch default"));
                    }
                    seen_default = true;
                }
                SwitchPattern::Expr(expr) => {
                    let key = format!("{:?}", expr.kind);
                    if !seen_patterns.insert(key) {
                        self.diagnostics
                            .push(Diagnostic::error(arm.span, "duplicate switch pattern"));
                    }
                }
            }
            all_arms_produce &= self.switch_tail_arm_produces_value(&arm.body);
        }
        all_arms_produce
    }

    fn switch_tail_arm_produces_value(&mut self, body: &SwitchArmBody) -> bool {
        match body {
            SwitchArmBody::Expr(_) => true,
            SwitchArmBody::Stmt(stmt) => !self.check_stmt(stmt).falls_through,
            SwitchArmBody::Block(block) => self.block_returns_on_all_paths(block),
        }
    }

    fn block_returns_on_all_paths(&mut self, block: &Block) -> bool {
        let flow = self.check_block(block);
        !flow.falls_through
            || block
                .tail
                .as_deref()
                .is_some_and(|tail| self.tail_expr_returns_on_all_paths(tail))
    }

    fn check_stmt(&mut self, stmt: &Stmt) -> Flow {
        match &stmt.kind {
            StmtKind::Binding(binding) => binding.value.as_ref().map_or(
                Flow {
                    falls_through: true,
                },
                |value| self.check_expr_flow(value),
            ),
            StmtKind::Expr(expr) => self.check_expr_flow(expr),
            StmtKind::Using(_) => Flow {
                falls_through: true,
            },
            StmtKind::Defer(expr) => {
                self.check_defer(expr);
                Flow {
                    falls_through: true,
                }
            }
            StmtKind::Return(_) => Flow {
                falls_through: false,
            },
            StmtKind::Break | StmtKind::Continue => {
                if self.loop_depth == 0 {
                    self.diagnostics.push(Diagnostic::error(
                        stmt.span,
                        "`break` and `continue` can only appear inside loops",
                    ));
                }
                Flow {
                    falls_through: false,
                }
            }
            StmtKind::For(for_stmt) => {
                if let ForHeader::CStyle { init, .. } = &for_stmt.header
                    && let Some(init) = init
                    && let nia_ast::ForInit::Binding { binding, .. } = &**init
                    && binding.value.is_none()
                {
                    self.diagnostics.push(Diagnostic::error(
                        stmt.span,
                        "for init binding declaration requires an initializer",
                    ));
                }
                self.loop_depth += 1;
                self.check_block(&for_stmt.body);
                self.loop_depth -= 1;
                Flow {
                    falls_through: true,
                }
            }
        }
    }

    fn check_expr_flow(&mut self, expr: &Expr) -> Flow {
        match &expr.kind {
            ExprKind::Block(block) => self.check_block(block),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.check_expr_flow(cond);
                let then_flow = self.check_block(then_branch);
                let else_flow = else_branch.as_deref().map_or(
                    Flow {
                        falls_through: true,
                    },
                    |else_branch| self.check_expr_flow(else_branch),
                );
                Flow {
                    falls_through: then_flow.falls_through || else_flow.falls_through,
                }
            }
            ExprKind::Switch(switch) => {
                self.check_switch_patterns(switch);
                self.check_expr_flow(&switch.target);
                let mut has_default = false;
                let mut all_arms_terminate = !switch.arms.is_empty();
                for arm in &switch.arms {
                    if matches!(arm.pattern, SwitchPattern::Default) {
                        has_default = true;
                    }
                    if let SwitchPattern::Expr(pattern) = &arm.pattern {
                        self.check_expr_flow(pattern);
                    }
                    all_arms_terminate &= !self.check_switch_arm_flow(&arm.body).falls_through;
                }
                Flow {
                    falls_through: !(has_default && all_arms_terminate),
                }
            }
            ExprKind::BracketSuffix { callee, args } => {
                self.check_expr_flow(callee);
                for arg in args {
                    if let Some(expr) = &arg.expr {
                        self.check_expr_flow(expr);
                    }
                }
                Flow {
                    falls_through: true,
                }
            }
            ExprKind::ArrayLiteral { elems } => {
                match elems {
                    nia_ast::ArrayElements::List(elems) => {
                        for elem in elems {
                            self.check_expr_flow(elem);
                        }
                    }
                    nia_ast::ArrayElements::Repeat { value, count } => {
                        self.check_expr_flow(value);
                        self.check_expr_flow(count);
                    }
                }
                Flow {
                    falls_through: true,
                }
            }
            ExprKind::StructLiteral { fields } => {
                for field in fields {
                    self.check_expr_flow(&field.value);
                }
                Flow {
                    falls_through: true,
                }
            }
            ExprKind::Unary { expr, .. } | ExprKind::Cast { expr, .. } => {
                self.check_expr_flow(expr);
                Flow {
                    falls_through: true,
                }
            }
            ExprKind::Binary { lhs, rhs, .. } | ExprKind::Assign { lhs, rhs, .. } => {
                self.check_expr_flow(lhs);
                self.check_expr_flow(rhs);
                Flow {
                    falls_through: true,
                }
            }
            ExprKind::Call { callee, args } => {
                self.check_expr_flow(callee);
                for arg in args {
                    self.check_expr_flow(arg);
                }
                Flow {
                    falls_through: true,
                }
            }
            ExprKind::Field { lhs, .. } => {
                self.check_expr_flow(lhs);
                Flow {
                    falls_through: true,
                }
            }
            ExprKind::Index { lhs, index } => {
                self.check_expr_flow(lhs);
                match index {
                    IndexArg::Expr(index) => {
                        self.check_expr_flow(index);
                    }
                    IndexArg::Range(range) => {
                        if let Some(start) = &range.start {
                            self.check_expr_flow(start);
                        }
                        if let Some(end) = &range.end {
                            self.check_expr_flow(end);
                        }
                    }
                }
                Flow {
                    falls_through: true,
                }
            }
            ExprKind::Error
            | ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::ByteString(_)
            | ExprKind::CString(_)
            | ExprKind::Char(_)
            | ExprKind::ByteChar(_)
            | ExprKind::Raw(_)
            | ExprKind::Bool(_)
            | ExprKind::Ident(_)
            | ExprKind::Underscore
            | ExprKind::Builtin { .. }
            | ExprKind::TypeTarget { .. }
            | ExprKind::Qualified { .. } => Flow {
                falls_through: true,
            },
        }
    }

    fn check_switch_arm_flow(&mut self, body: &SwitchArmBody) -> Flow {
        match body {
            SwitchArmBody::Expr(expr) => self.check_expr_flow(expr),
            SwitchArmBody::Stmt(stmt) => self.check_stmt(stmt),
            SwitchArmBody::Block(block) => self.check_block(block),
        }
    }

    fn check_defer(&mut self, expr: &Expr) {
        self.check_no_deferred_control_flow(expr);
    }

    fn check_no_deferred_control_flow(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Error
            | ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::ByteString(_)
            | ExprKind::CString(_)
            | ExprKind::Char(_)
            | ExprKind::ByteChar(_)
            | ExprKind::Raw(_)
            | ExprKind::Bool(_)
            | ExprKind::Ident(_)
            | ExprKind::Underscore
            | ExprKind::Builtin { .. }
            | ExprKind::TypeTarget { .. }
            | ExprKind::Qualified { .. } => {}
            ExprKind::BracketSuffix { callee, args } => {
                self.check_no_deferred_control_flow(callee);
                for arg in args {
                    if let Some(expr) = &arg.expr {
                        self.check_no_deferred_control_flow(expr);
                    }
                }
            }
            ExprKind::ArrayLiteral { elems } => match elems {
                nia_ast::ArrayElements::List(elems) => {
                    for elem in elems {
                        self.check_no_deferred_control_flow(elem);
                    }
                }
                nia_ast::ArrayElements::Repeat { value, count } => {
                    self.check_no_deferred_control_flow(value);
                    self.check_no_deferred_control_flow(count);
                }
            },
            ExprKind::StructLiteral { fields } => {
                for field in fields {
                    self.check_no_deferred_control_flow(&field.value);
                }
            }
            ExprKind::Unary { expr, .. } | ExprKind::Cast { expr, .. } => {
                self.check_no_deferred_control_flow(expr);
            }
            ExprKind::Binary { lhs, rhs, .. } | ExprKind::Assign { lhs, rhs, .. } => {
                self.check_no_deferred_control_flow(lhs);
                self.check_no_deferred_control_flow(rhs);
            }
            ExprKind::Call { callee, args } => {
                self.check_no_deferred_control_flow(callee);
                for arg in args {
                    self.check_no_deferred_control_flow(arg);
                }
            }
            ExprKind::Field { lhs, .. } => self.check_no_deferred_control_flow(lhs),
            ExprKind::Index { lhs, index } => {
                self.check_no_deferred_control_flow(lhs);
                match index {
                    IndexArg::Expr(index) => self.check_no_deferred_control_flow(index),
                    IndexArg::Range(range) => {
                        if let Some(start) = &range.start {
                            self.check_no_deferred_control_flow(start);
                        }
                        if let Some(end) = &range.end {
                            self.check_no_deferred_control_flow(end);
                        }
                    }
                }
            }
            ExprKind::Block(block) => self.check_no_deferred_control_flow_in_block(block),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.check_no_deferred_control_flow(cond);
                self.check_no_deferred_control_flow_in_block(then_branch);
                if let Some(else_branch) = else_branch {
                    self.check_no_deferred_control_flow(else_branch);
                }
            }
            ExprKind::Switch(switch) => {
                self.check_switch_patterns(switch);
                self.check_no_deferred_control_flow(&switch.target);
                for arm in &switch.arms {
                    if let SwitchPattern::Expr(expr) = &arm.pattern {
                        self.check_no_deferred_control_flow(expr);
                    }
                    match &arm.body {
                        SwitchArmBody::Expr(expr) => self.check_no_deferred_control_flow(expr),
                        SwitchArmBody::Stmt(stmt) => {
                            self.check_no_deferred_control_flow_in_block(&Block {
                                span: stmt.span,
                                stmts: vec![*stmt.clone()],
                                tail: None,
                            });
                        }
                        SwitchArmBody::Block(block) => {
                            self.check_no_deferred_control_flow_in_block(block);
                        }
                    }
                }
            }
        }
    }

    fn check_no_deferred_control_flow_in_block(&mut self, block: &Block) {
        for stmt in &block.stmts {
            match &stmt.kind {
                StmtKind::Binding(binding) => {
                    if let Some(value) = &binding.value {
                        self.check_no_deferred_control_flow(value);
                    }
                }
                StmtKind::Expr(expr) | StmtKind::Defer(expr) => {
                    self.check_no_deferred_control_flow(expr);
                }
                StmtKind::Using(_) => {}
                StmtKind::Return(_) => self.diagnostics.push(Diagnostic::error(
                    stmt.span,
                    "`return` is not allowed inside deferred expressions",
                )),
                StmtKind::Break => self.diagnostics.push(Diagnostic::error(
                    stmt.span,
                    "`break` is not allowed inside deferred expressions",
                )),
                StmtKind::Continue => self.diagnostics.push(Diagnostic::error(
                    stmt.span,
                    "`continue` is not allowed inside deferred expressions",
                )),
                StmtKind::For(for_stmt) => {
                    if let ForHeader::CStyle { init, cond, step } = &for_stmt.header {
                        if let Some(init) = init {
                            match &**init {
                                nia_ast::ForInit::Binding { binding, .. } => {
                                    if let Some(value) = &binding.value {
                                        self.check_no_deferred_control_flow(value);
                                    }
                                }
                                nia_ast::ForInit::Expr(expr) => {
                                    self.check_no_deferred_control_flow(expr);
                                }
                            }
                        }
                        if let Some(cond) = cond {
                            self.check_no_deferred_control_flow(cond);
                        }
                        if let Some(step) = step {
                            self.check_no_deferred_control_flow(step);
                        }
                    } else if let ForHeader::Condition(cond) = &for_stmt.header {
                        self.check_no_deferred_control_flow(cond);
                    }
                    self.check_no_deferred_control_flow_in_block(&for_stmt.body);
                }
            }
        }
        if let Some(tail) = &block.tail {
            self.check_no_deferred_control_flow(tail);
        }
    }

    fn check_switch_patterns(&mut self, switch: &nia_ast::SwitchStmt) {
        let mut has_default = false;
        let mut seen_patterns = HashSet::new();
        for arm in &switch.arms {
            match &arm.pattern {
                SwitchPattern::Default => {
                    if has_default {
                        self.diagnostics
                            .push(Diagnostic::error(arm.span, "duplicate switch default"));
                    }
                    has_default = true;
                }
                SwitchPattern::Expr(expr) => {
                    let key = format!("{:?}", expr.kind);
                    if !seen_patterns.insert(key) {
                        self.diagnostics
                            .push(Diagnostic::error(arm.span, "duplicate switch pattern"));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{ModuleId, collect_module_defs};
    use nia_item_signatures::collect_item_signatures;
    use nia_parser::parse_module;
    use nia_type_lower::lower_module_types_with_id;
    use nia_type_resolve::resolve_module_types;

    fn pipeline(source: &str) -> FlowCheck {
        let (module, parse_errors) = parse_module(source);
        assert!(parse_errors.is_empty(), "{parse_errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types_with_id(ModuleId(0), &module, &resolved);
        let signatures = collect_item_signatures(&module, &defs, &lowered);
        check_module_flow(&module, &lowered.interner, &signatures)
    }

    #[test]
    fn rejects_break_and_continue_outside_loops() {
        let checked = pipeline(
            r#"
fn main() {
    break;
    continue;
}
"#,
        );
        assert_eq!(
            checked
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.message.contains("inside loops"))
                .count(),
            2
        );
    }

    #[test]
    fn reports_missing_returns_and_unreachable_statements() {
        let checked = pipeline(
            r#"
fn a(flag: bool) i32 {
    if flag {
        return 1;
    }
}

fn b() i32 {
    return 1;
    var x = 2;
}
"#,
        );
        assert!(checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("does not return on all reachable paths")
        }));
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("unreachable statement"))
        );
    }

    #[test]
    fn checks_switch_duplicate_default_and_patterns() {
        let checked = pipeline(
            r#"
fn main(x: i32) {
    switch x {
        1 => return,
        1 => return,
        _ => return,
        _ => return,
    }
}
"#,
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("duplicate switch pattern"))
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("duplicate switch default"))
        );
    }

    #[test]
    fn accepts_empty_block_tail_for_empty_struct_return() {
        let checked = pipeline(
            r#"
struct Empty {}

fn make() Empty {
    {}
}
"#,
        );
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }

    #[test]
    fn exhaustive_switch_returns_on_all_paths() {
        let checked = pipeline(
            r#"
fn name(x: u32) &const u8 {
    switch x {
        1 => return 0 as &const u8,
        _ => return 1 as &const u8,
    }
}
"#,
        );
        assert!(
            !checked.diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("does not return on all reachable paths")),
            "{:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn switch_tail_expression_satisfies_return_analysis() {
        let checked = pipeline(
            r#"
fn name(x: u32) &const u8 {
    switch x {
        1 => 0 as &const u8,
        _ => 1 as &const u8,
    }
}
"#,
        );
        assert!(
            !checked.diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("does not return on all reachable paths")),
            "{:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn exhaustive_switch_tail_without_default_satisfies_return_analysis() {
        let checked = pipeline(
            r#"
enum Mode: u8 {
    A,
    B,
}

fn name(mode: Mode) u32 {
    switch mode {
        Mode::A => 1,
        Mode::B => 2,
    }
}
"#,
        );
        assert!(
            !checked.diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("does not return on all reachable paths")),
            "{:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn accepts_deferred_blocks_but_rejects_deferred_control_flow() {
        let checked = pipeline(
            r#"
fn cleanup() {}

fn main() {
    defer {
        cleanup();
    };
    defer {
        return;
    };
}
"#,
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("`return` is not allowed")),
            "{:?}",
            checked.diagnostics
        );
        assert!(
            !checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("requires a call")),
            "{:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn rejects_all_control_flow_inside_deferred_expressions() {
        let checked = pipeline(
            r#"
fn cleanup() {}

fn main() {
    defer {
        if true {
            return;
        }
    };
    for {
        defer {
            break;
        };
        defer {
            continue;
        };
        break;
    }
    defer {
        switch 1 {
            1 => return,
            _ => cleanup(),
        }
    };
}
"#,
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("`return` is not allowed")),
            "{:?}",
            checked.diagnostics
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("`break` is not allowed")),
            "{:?}",
            checked.diagnostics
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("`continue` is not allowed")),
            "{:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn rejects_control_flow_deep_inside_deferred_expression_shapes() {
        let checked = pipeline(
            r#"
fn cleanup() {}

fn main(flag: bool) {
    defer if flag {
        for {
            continue;
        }
    } else {
        cleanup();
    };

    defer {
        switch 1 {
            0 => {
                for {
                    break;
                }
            },
            _ => cleanup(),
        }
    };
}
"#,
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("`continue` is not allowed")),
            "{:?}",
            checked.diagnostics
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("`break` is not allowed")),
            "{:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn accepts_nested_loop_control_flow_outside_defer() {
        let checked = pipeline(
            r#"
fn main(limit: i32) {
    var i = 0;
    for ; i < limit; i += 1 {
        if i == 1 {
            continue;
        }
        for {
            break;
        }
    }
}
"#,
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .all(|diagnostic| !diagnostic.message.contains("inside loops")),
            "{:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn reports_unreachable_after_switch_arm_control_flow_blocks() {
        let checked = pipeline(
            r#"
fn main(kind: i32) {
    switch kind {
        0 => return,
        _ => {
            return;
        },
    }
    var unreachable = 1;
}
"#,
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("unreachable statement")),
            "{:?}",
            checked.diagnostics
        );
    }
}
