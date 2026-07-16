// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{
    Block, Expr, ExprKind, FunctionItem, IndexArg, Module, Pattern, PatternKind, Stmt, StmtKind,
    SwitchArmBody, SwitchPattern, SwitchPatternKind,
};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{DefId, GlobalDefId, ModuleId};
use nia_item_signatures::{FunctionSignature, ItemSignatures};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNode, ItemTreeNodeKind, ModuleItemTree};
use nia_symbol::SymbolId;
use nia_ty::{PrimitiveTy, TyKind, TypeStore};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub struct FlowCheck {
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy)]
pub struct FlowCheckSignatures<'a> {
    pub functions: &'a std::collections::HashMap<DefId, FunctionSignature>,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum FlowCheckFilter<'a> {
    #[default]
    All,
    ReachableFunctions {
        module_id: ModuleId,
        functions: &'a HashSet<GlobalDefId>,
    },
}

impl FlowCheckFilter<'_> {
    fn includes(self, def_id: DefId) -> bool {
        match self {
            Self::All => true,
            Self::ReachableFunctions {
                module_id,
                functions,
            } => functions.contains(&GlobalDefId { module_id, def_id }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Flow {
    falls_through: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum SwitchPatternFingerprint {
    Expr(ExprFingerprint),
    Range {
        start: ExprFingerprint,
        end: ExprFingerprint,
        inclusive: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ExprFingerprint {
    Integer(String),
    Float(String),
    String(Vec<String>),
    ByteString(Vec<String>),
    Char(String),
    ByteChar(String),
    Bool(bool),
    Null,
    Ident(SymbolId),
    Qualified(Box<ExprFingerprint>, SymbolId),
}

pub fn check_module_flow(
    module: &Module,
    type_store: &TypeStore,
    signatures: &ItemSignatures,
) -> FlowCheck {
    let item_tree = ModuleItemTree::from_module(module);
    let active_item_tree =
        ActiveModuleItemTree::new(item_tree.active_items_without_const(), HashSet::new());
    check_active_module_flow(&active_item_tree, type_store, signatures)
}

pub fn check_active_module_flow(
    item_tree: &ActiveModuleItemTree,
    type_store: &TypeStore,
    signatures: &ItemSignatures,
) -> FlowCheck {
    check_active_module_flow_with_signatures(
        item_tree,
        type_store,
        FlowCheckSignatures {
            functions: &signatures.functions,
        },
    )
}

pub fn check_active_module_flow_with_signatures(
    item_tree: &ActiveModuleItemTree,
    type_store: &TypeStore,
    signatures: FlowCheckSignatures<'_>,
) -> FlowCheck {
    check_active_module_flow_with_signatures_and_filter(
        item_tree,
        type_store,
        signatures,
        FlowCheckFilter::All,
    )
}

pub fn check_active_module_flow_with_signatures_and_filter(
    item_tree: &ActiveModuleItemTree,
    type_store: &TypeStore,
    signatures: FlowCheckSignatures<'_>,
    filter: FlowCheckFilter<'_>,
) -> FlowCheck {
    let mut checker = FlowChecker {
        type_store,
        signatures,
        filter,
        diagnostics: Vec::new(),
        loop_depth: 0,
    };
    checker.check_active_module(item_tree);
    FlowCheck {
        diagnostics: checker.diagnostics,
    }
}

struct FlowChecker<'a> {
    type_store: &'a TypeStore,
    signatures: FlowCheckSignatures<'a>,
    filter: FlowCheckFilter<'a>,
    diagnostics: Vec<Diagnostic>,
    loop_depth: usize,
}

impl FlowChecker<'_> {
    fn check_active_module(&mut self, item_tree: &ActiveModuleItemTree) {
        self.check_items(&item_tree.items);
    }

    fn check_items(&mut self, items: &[ItemTreeNode]) {
        for item in items {
            match &item.kind {
                ItemTreeNodeKind::Function(function) => self.check_function(function),
                ItemTreeNodeKind::Trait(item_trait) => {
                    for method in &item_trait.methods {
                        self.check_function(&method.function);
                    }
                }
                ItemTreeNodeKind::Extend(extend) => {
                    for method in &extend.methods {
                        self.check_function(&method.function);
                    }
                }
                ItemTreeNodeKind::Module(_)
                | ItemTreeNodeKind::Using(_)
                | ItemTreeNodeKind::Struct(_)
                | ItemTreeNodeKind::Union(_)
                | ItemTreeNodeKind::Enum(_)
                | ItemTreeNodeKind::TypeAlias(_)
                | ItemTreeNodeKind::Binding(_) => {}
            }
        }
    }

    fn check_function(&mut self, function: &FunctionItem) {
        let signature = self.signature_for_function(function);
        if let Some((def_id, _)) = signature
            && !self.filter.includes(def_id)
        {
            return;
        }
        let Some(body) = &function.body else {
            return;
        };
        let flow = self.check_block(body);
        let tail_returns = body
            .tail
            .as_deref()
            .is_some_and(|tail| self.tail_expr_returns_on_all_paths(tail));
        if self.function_requires_return(function) && flow.falls_through && !tail_returns {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                body.span,
                "non-void function does not return on all reachable paths",
            ));
        }
    }

    fn function_requires_return(&self, function: &FunctionItem) -> bool {
        let Some((_, signature)) = self.signature_for_function(function) else {
            return false;
        };
        !matches!(
            self.type_store.get(signature.return_type),
            Some(TyKind::Primitive(PrimitiveTy::Void))
        )
    }

    fn signature_for_function(
        &self,
        function: &FunctionItem,
    ) -> Option<(DefId, &FunctionSignature)> {
        self.signatures
            .functions
            .iter()
            .find_map(|(def_id, signature)| {
                (signature.span == function.span).then_some((*def_id, signature))
            })
    }

    fn check_block(&mut self, block: &Block) -> Flow {
        let mut falls_through = true;
        for stmt in &block.stmts {
            if !falls_through {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::STATIC_CHECK,
                    stmt.span,
                    "unreachable statement",
                ));
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
        self.check_switch_patterns(switch);
        let mut all_arms_produce = !switch.arms.is_empty();
        for arm in &switch.arms {
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
            StmtKind::Static(binding) => binding.value.as_ref().map_or(
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
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::STATIC_CHECK,
                        stmt.span,
                        "`break` and `continue` can only appear inside loops",
                    ));
                }
                Flow {
                    falls_through: false,
                }
            }
            StmtKind::ForIn(for_stmt) => {
                self.check_expr_flow(&for_stmt.iter);
                self.loop_depth += 1;
                self.check_block(&for_stmt.body);
                self.loop_depth -= 1;
                Flow {
                    falls_through: true,
                }
            }
            StmtKind::While(while_stmt) => {
                self.check_expr_flow(&while_stmt.cond);
                self.loop_depth += 1;
                self.check_block(&while_stmt.body);
                self.loop_depth -= 1;
                Flow {
                    falls_through: true,
                }
            }
            StmtKind::Loop(loop_stmt) => {
                self.loop_depth += 1;
                self.check_block(&loop_stmt.body);
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
            ExprKind::IfPattern(if_pattern) => {
                self.check_expr_flow(&if_pattern.target);
                let mut falls_through = if_pattern.else_branch.is_none();
                for arm in &if_pattern.arms {
                    self.check_pattern_flow(&arm.pattern);
                    falls_through |= self.check_block(&arm.body).falls_through;
                }
                if let Some(else_branch) = &if_pattern.else_branch {
                    falls_through |= self.check_expr_flow(else_branch).falls_through;
                }
                Flow { falls_through }
            }
            ExprKind::Switch(switch) => {
                self.check_switch_patterns(switch);
                self.check_expr_flow(&switch.target);
                let mut has_default = false;
                let mut all_arms_terminate = !switch.arms.is_empty();
                for arm in &switch.arms {
                    for pattern in &arm.patterns {
                        if matches!(&pattern.kind, SwitchPatternKind::Wildcard) {
                            has_default = true;
                        }
                        self.check_switch_pattern_flow(pattern);
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
            ExprKind::ArrayLiteral { elems } | ExprKind::TypedArrayLiteral { elems, .. } => {
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
            ExprKind::StructLiteral { fields } | ExprKind::TypedStructLiteral { fields, .. } => {
                for field in fields {
                    self.check_expr_flow(&field.value);
                }
                Flow {
                    falls_through: true,
                }
            }
            ExprKind::Unary { expr, .. }
            | ExprKind::OptionalSome { expr }
            | ExprKind::ErrorOk { expr }
            | ExprKind::ErrorErr { expr }
            | ExprKind::Try { expr }
            | ExprKind::Cast { expr, .. } => {
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
            ExprKind::Range(range) => {
                if let Some(start) = &range.start {
                    self.check_expr_flow(start);
                }
                if let Some(end) = &range.end {
                    self.check_expr_flow(end);
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
            | ExprKind::Char(_)
            | ExprKind::ByteChar(_)
            | ExprKind::Raw(_)
            | ExprKind::Bool(_)
            | ExprKind::Null
            | ExprKind::Ident(_)
            | ExprKind::SelfValue
            | ExprKind::PathRoot(_)
            | ExprKind::Underscore
            | ExprKind::TypeTarget { .. }
            | ExprKind::TraitTarget { .. }
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
        self.check_expr_flow(expr);
    }

    fn check_switch_pattern_flow(&mut self, pattern: &SwitchPattern) {
        match &pattern.kind {
            SwitchPatternKind::Wildcard => {}
            SwitchPatternKind::Expr(expr) => {
                self.check_expr_flow(expr);
            }
            SwitchPatternKind::Range { start, end, .. } => {
                self.check_expr_flow(start);
                self.check_expr_flow(end);
            }
        }
    }

    fn check_pattern_flow(&mut self, pattern: &Pattern) {
        match &pattern.kind {
            PatternKind::Wildcard | PatternKind::Bind { .. } | PatternKind::OptionalNull => {}
            PatternKind::Pointer(pattern)
            | PatternKind::MutPointer(pattern)
            | PatternKind::OptionalSome(pattern)
            | PatternKind::ErrorOk(pattern)
            | PatternKind::ErrorErr(pattern) => self.check_pattern_flow(pattern),
            PatternKind::Expr(expr) => {
                self.check_expr_flow(expr);
            }
            PatternKind::Range { start, end, .. } => {
                self.check_expr_flow(start);
                self.check_expr_flow(end);
            }
        }
    }

    fn check_switch_patterns(&mut self, switch: &nia_ast::SwitchStmt) {
        let mut has_default = false;
        let mut seen = HashSet::new();
        for arm in &switch.arms {
            for pattern in &arm.patterns {
                if matches!(&pattern.kind, SwitchPatternKind::Wildcard) {
                    if has_default {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::STATIC_CHECK,
                            arm.span,
                            "duplicate switch default",
                        ));
                    }
                    has_default = true;
                    continue;
                }
                if let Some(fingerprint) = Self::switch_pattern_fingerprint(pattern)
                    && !seen.insert(fingerprint)
                {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::STATIC_CHECK,
                        pattern.span,
                        "duplicate switch pattern",
                    ));
                }
            }
        }
    }

    fn switch_pattern_fingerprint(pattern: &SwitchPattern) -> Option<SwitchPatternFingerprint> {
        match &pattern.kind {
            SwitchPatternKind::Wildcard => None,
            SwitchPatternKind::Expr(expr) => Some(SwitchPatternFingerprint::Expr(
                Self::expr_fingerprint(expr)?,
            )),
            SwitchPatternKind::Range {
                start,
                end,
                inclusive,
            } => Some(SwitchPatternFingerprint::Range {
                start: Self::expr_fingerprint(start)?,
                end: Self::expr_fingerprint(end)?,
                inclusive: *inclusive,
            }),
        }
    }

    fn expr_fingerprint(expr: &Expr) -> Option<ExprFingerprint> {
        match &expr.kind {
            ExprKind::Integer(value) => Some(ExprFingerprint::Integer(value.clone())),
            ExprKind::Float(value) => Some(ExprFingerprint::Float(value.clone())),
            ExprKind::String(value) => Some(ExprFingerprint::String(value.parts.clone())),
            ExprKind::ByteString(value) => Some(ExprFingerprint::ByteString(value.parts.clone())),
            ExprKind::Char(value) => Some(ExprFingerprint::Char(value.clone())),
            ExprKind::ByteChar(value) => Some(ExprFingerprint::ByteChar(value.clone())),
            ExprKind::Bool(value) => Some(ExprFingerprint::Bool(*value)),
            ExprKind::Null => Some(ExprFingerprint::Null),
            ExprKind::Ident(name) => Some(ExprFingerprint::Ident(*name)),
            ExprKind::Qualified { lhs, name } => Some(ExprFingerprint::Qualified(
                Box::new(Self::expr_fingerprint(lhs)?),
                *name,
            )),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{ModuleId, collect_module_defs};
    use nia_item_signatures::{ItemSignatureInput, ItemSignatureSource, collect_item_signatures};
    use nia_parser::parse_module;
    use nia_type_lower::{TypeLoweringContext, lower_module_types_with_context};
    use nia_type_resolve::resolve_module_types;

    fn pipeline(source: &str) -> FlowCheck {
        let (module, parse_errors) = parse_module(source);
        assert!(parse_errors.is_empty(), "{parse_errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let type_store = TypeStore::new();
        let lowered = lower_module_types_with_context(
            ModuleId(0),
            &module,
            &resolved,
            TypeLoweringContext::empty(&type_store),
        );
        let signatures = collect_item_signatures(ItemSignatureInput {
            source: ItemSignatureSource::Module(&module),
            defs: &defs,
            lowered: &lowered,
            type_store: &type_store,
            symbols: None,
        });
        check_module_flow(&module, &type_store, &signatures)
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
                .filter(|diagnostic| diagnostic.summary.contains("inside loops"))
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
    let mut x = 2;
}
"#,
        );
        assert!(checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("does not return on all reachable paths")
        }));
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("unreachable statement"))
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
                .any(|diagnostic| diagnostic.summary.contains("duplicate switch pattern"))
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("duplicate switch default"))
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
fn name(x: u32) &u8 {
    switch x {
        1 => return 0 as &u8,
        _ => return 1 as &u8,
    }
}
"#,
        );
        assert!(
            !checked.diagnostics.iter().any(|diagnostic| diagnostic
                .summary
                .contains("does not return on all reachable paths")),
            "{:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn switch_tail_expression_satisfies_return_analysis() {
        let checked = pipeline(
            r#"
fn name(x: u32) &u8 {
    switch x {
        1 => 0 as &u8,
        _ => 1 as &u8,
    }
}
"#,
        );
        assert!(
            !checked.diagnostics.iter().any(|diagnostic| diagnostic
                .summary
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
                .summary
                .contains("does not return on all reachable paths")),
            "{:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn accepts_deferred_blocks_and_return_control_flow() {
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
            !checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("requires a call")),
            "{:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn accepts_deferred_loop_control_flow_inside_loops() {
        let checked = pipeline(
            r#"
fn cleanup() {}

fn main() {
    defer {
        if true {
            return;
        }
    };
    loop {
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
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }

    #[test]
    fn rejects_deferred_break_and_continue_outside_loop_context() {
        let checked = pipeline(
            r#"
fn cleanup() {}

fn bad_continue(flag: bool) {
    defer if flag {
        continue;
    } else {
        cleanup();
    };
}

fn bad_break() {
    defer {
        switch 1 {
            0 => {
                break;
            },
            _ => cleanup(),
        }
    };
}
"#,
        );
        assert_eq!(
            checked
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic
                    .summary
                    .contains("`break` and `continue` can only appear inside loops"))
                .count(),
            1,
            "{:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn accepts_deferred_break_and_continue_inside_nested_loops() {
        let checked = pipeline(
            r#"
fn main(flag: bool) {
    loop {
        defer if flag {
            loop {
                continue;
            }
        } else {
            break;
        };
        break;
    }
}
"#,
        );
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }

    #[test]
    fn accepts_nested_loop_control_flow_outside_defer() {
        let checked = pipeline(
            r#"
fn main(limit: i32) {
    for i in 0..limit {
        if i == 1 {
            continue;
        }
        loop {
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
                .all(|diagnostic| !diagnostic.summary.contains("inside loops")),
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
    let mut unreachable = 1;
}
"#,
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("unreachable statement")),
            "{:?}",
            checked.diagnostics
        );
    }
}
