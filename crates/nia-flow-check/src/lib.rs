// SPDX-License-Identifier: GPL-3.0-or-later
//! Conservative syntax-level control-flow diagnostics.
//!
//! This crate finds unreachable statements, invalid loop control, duplicate
//! patterns, and missing returns that can be established without typed pattern
//! semantics. Exhaustiveness over nominal constructors and scalar ranges is
//! deliberately owned by the typed pattern matrix in `nia-body-check`.

use nia_ast::{
    Block, Expr, ExprKind, FunctionItem, IndexArg, MatchArmBody, Module, Pattern, PatternKind,
    Stmt, StmtKind,
};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{DefId, GlobalDefId, ModuleId};
use nia_item_signatures::{FunctionSignature, ItemSignatures};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeItems, ItemTreeNodeKind, ModuleItemTree};
use nia_symbol::SymbolId;
use nia_ty::{TyKind, TypeStore};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
/// Diagnostics produced by one module flow-check pass.
pub struct FlowCheck {
    /// User-facing flow diagnostics in traversal order.
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy)]
/// Signature subset required to classify function return types.
pub struct FlowCheckSignatures<'a> {
    /// Function signatures keyed by module-local definition identity.
    pub functions: &'a std::collections::HashMap<DefId, FunctionSignature>,
}

#[derive(Debug, Clone, Copy, Default)]
/// Selects which source functions participate in flow checking.
pub enum FlowCheckFilter<'a> {
    /// Checks every active function with a body.
    #[default]
    All,
    /// Checks only functions selected by executable reachability.
    ReachableFunctions {
        /// Module owning the local definition identities.
        module_id: ModuleId,
        /// Program-wide set of reachable function identities.
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
    returns: bool,
    breaks_loop: bool,
    continues_loop: bool,
}

impl Flow {
    const NONE: Self = Self {
        falls_through: false,
        returns: false,
        breaks_loop: false,
        continues_loop: false,
    };
    const NEXT: Self = Self {
        falls_through: true,
        ..Self::NONE
    };
    const RETURN: Self = Self {
        returns: true,
        ..Self::NONE
    };
    const BREAK: Self = Self {
        breaks_loop: true,
        ..Self::NONE
    };
    const CONTINUE: Self = Self {
        continues_loop: true,
        ..Self::NONE
    };

    fn union(self, other: Self) -> Self {
        Self {
            falls_through: self.falls_through || other.falls_through,
            returns: self.returns || other.returns,
            breaks_loop: self.breaks_loop || other.breaks_loop,
            continues_loop: self.continues_loop || other.continues_loop,
        }
    }

    /// Runs `next` on every normally completing path while retaining exits
    /// already taken by `self`.
    fn then(self, next: Self) -> Self {
        Self {
            falls_through: self.falls_through && next.falls_through,
            returns: self.returns || self.falls_through && next.returns,
            breaks_loop: self.breaks_loop || self.falls_through && next.breaks_loop,
            continues_loop: self.continues_loop || self.falls_through && next.continues_loop,
        }
    }

    fn without_fallthrough(self) -> Self {
        Self {
            falls_through: false,
            ..self
        }
    }

    /// A deferred expression replaces the exit that invoked it when it takes
    /// control itself; normal cleanup completion preserves the original exit.
    fn through_defer(self, deferred: Self) -> Self {
        deferred
            .without_fallthrough()
            .union(if deferred.falls_through {
                self
            } else {
                Self::NONE
            })
    }

    fn through_defers(mut self, defers: &[Self]) -> Self {
        for deferred in defers.iter().rev() {
            self = self.through_defer(*deferred);
        }
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum PatternFingerprint {
    Pointer(Box<PatternFingerprint>),
    MutPointer(Box<PatternFingerprint>),
    OptionalSome(Box<PatternFingerprint>),
    OptionalNull,
    ErrorOk(Box<PatternFingerprint>),
    ErrorErr(Box<PatternFingerprint>),
    Tuple(Vec<PatternFingerprint>),
    EnumVariant {
        variant: ExprFingerprint,
        fields: Vec<(Option<SymbolId>, PatternFingerprint)>,
    },
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

/// Conservative syntactic facts used only for control-flow joins.
///
/// This tracker intentionally proves only shapes that are independent of
/// semantic type information (wildcards, bindings, tuples, optionals, and
/// error-union constructors). Nominal constructors, literal intervals, and
/// enum coverage belong to `nia-body-check::patterns`, whose typed matrix has
/// the constructor universe needed for a sound exhaustiveness decision.
#[derive(Default)]
struct SyntacticPatternCoverage {
    catch_all: bool,
    optional_some: Option<Box<SyntacticPatternCoverage>>,
    optional_null: bool,
    error_ok: Option<Box<SyntacticPatternCoverage>>,
    error_err: Option<Box<SyntacticPatternCoverage>>,
}

impl SyntacticPatternCoverage {
    fn is_catch_all(pattern: &Pattern) -> bool {
        match &pattern.kind {
            PatternKind::Wildcard | PatternKind::Bind { .. } => true,
            PatternKind::Pointer(inner) | PatternKind::MutPointer(inner) => {
                Self::is_catch_all(inner)
            }
            PatternKind::Tuple(fields) => fields.iter().all(Self::is_catch_all),
            PatternKind::OptionalSome(_)
            | PatternKind::OptionalNull
            | PatternKind::ErrorOk(_)
            | PatternKind::ErrorErr(_)
            | PatternKind::Nominal { .. }
            | PatternKind::Expr(_)
            | PatternKind::Range { .. } => false,
        }
    }

    fn record(&mut self, pattern: &Pattern) {
        match &pattern.kind {
            PatternKind::Wildcard | PatternKind::Bind { .. } => self.catch_all = true,
            PatternKind::Pointer(inner) | PatternKind::MutPointer(inner) => self.record(inner),
            PatternKind::OptionalSome(inner) => self
                .optional_some
                .get_or_insert_with(Default::default)
                .record(inner),
            PatternKind::OptionalNull => self.optional_null = true,
            PatternKind::ErrorOk(inner) => self
                .error_ok
                .get_or_insert_with(Default::default)
                .record(inner),
            PatternKind::ErrorErr(inner) => self
                .error_err
                .get_or_insert_with(Default::default)
                .record(inner),
            PatternKind::Tuple(fields) => {
                if fields.iter().all(Self::is_catch_all) {
                    self.catch_all = true;
                }
            }
            PatternKind::Nominal { .. } => {}
            PatternKind::Expr(_) | PatternKind::Range { .. } => {}
        }
    }

    fn covers_all(&self) -> bool {
        self.catch_all
            || (self.optional_null && self.optional_some.as_deref().is_some_and(Self::covers_all))
            || (self.error_ok.as_deref().is_some_and(Self::covers_all)
                && self.error_err.as_deref().is_some_and(Self::covers_all))
    }
}

/// Checks all non-const items in a parsed module.
///
/// Prefer [`check_active_module_flow`] when the caller already owns an active
/// item tree, since it preserves revision filtering performed by the query
/// layer.
pub fn check_module_flow(
    module: &Module,
    type_store: &TypeStore,
    signatures: &ItemSignatures,
) -> FlowCheck {
    let item_tree = ModuleItemTree::from_module(module);
    let active_item_tree = item_tree.all_items_active();
    check_active_module_flow(&active_item_tree, type_store, signatures)
}

/// Checks every function in an active item tree using full item signatures.
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

/// Checks every function using the minimal signature view required by flow.
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

/// Checks the functions accepted by `filter` using a minimal signature view.
///
/// Filtering applies only after a function is matched to its stable definition
/// identity. Items without signatures are still traversed for syntax-level
/// diagnostics, but cannot be diagnosed for a missing non-unit return.
pub fn check_active_module_flow_with_signatures_and_filter(
    item_tree: &ActiveModuleItemTree,
    type_store: &TypeStore,
    signatures: FlowCheckSignatures<'_>,
    filter: FlowCheckFilter<'_>,
) -> FlowCheck {
    let mut checker = FlowChecker {
        function_context: Some(FunctionFlowContext {
            type_store,
            signatures,
            filter,
        }),
        diagnostics: Vec::new(),
        loop_depth: 0,
        block_flows: HashMap::new(),
        stmt_flows: HashMap::new(),
    };
    checker.check_active_module(item_tree);
    FlowCheck {
        diagnostics: checker.diagnostics,
    }
}

/// Returns whether control can reach the end of `block` normally.
///
/// Return diagnostics and body typing share this definition, including
/// short-circuit paths, loop exits, and deferred control-flow overrides.
pub fn block_falls_through(block: &Block) -> bool {
    block_flow_summary(block)
        .get(&std::ptr::from_ref(block))
        .copied()
        .unwrap_or(true)
}

/// Computes normal-completion facts for `block` and every nested block in one
/// traversal. The raw pointers are only valid while the original AST remains
/// alive; callers should consume this map within that AST operation.
pub fn block_flow_summary(block: &Block) -> HashMap<*const Block, bool> {
    let mut checker = FlowChecker {
        function_context: None,
        diagnostics: Vec::new(),
        loop_depth: 0,
        block_flows: HashMap::new(),
        stmt_flows: HashMap::new(),
    };
    checker.check_block(block);
    checker
        .block_flows
        .into_iter()
        .map(|(block, flow)| (block, flow.falls_through))
        .collect()
}

#[derive(Clone, Copy)]
struct FunctionFlowContext<'a> {
    type_store: &'a TypeStore,
    signatures: FlowCheckSignatures<'a>,
    filter: FlowCheckFilter<'a>,
}

struct FlowChecker<'a> {
    function_context: Option<FunctionFlowContext<'a>>,
    diagnostics: Vec<Diagnostic>,
    loop_depth: usize,
    block_flows: HashMap<*const Block, Flow>,
    stmt_flows: HashMap<*const Stmt, Flow>,
}

impl FlowChecker<'_> {
    fn check_active_module(&mut self, item_tree: &ActiveModuleItemTree) {
        self.check_items(&item_tree.items);
    }

    fn check_items(&mut self, items: &ItemTreeItems) {
        for item in items.iter() {
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
            && self
                .function_context
                .is_some_and(|context| !context.filter.includes(def_id))
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
            let summary = "non-unit function does not return on all reachable paths";
            self.diagnostics.push(
                Diagnostic::user_error(codes::STATIC_CHECK, summary)
                    .primary(body.span, "this function body can reach its end without a value")
                    .secondary(
                        function.span,
                        "the function declaration requires a return value",
                    )
                    .note("every reachable path must return a value of the declared return type")
                    .help("add a return value on the missing paths, or change the function return type to `()`")
                    .finish(),
            );
        }
    }

    fn function_requires_return(&self, function: &FunctionItem) -> bool {
        let Some(context) = self.function_context else {
            return false;
        };
        let Some((_, signature)) = self.signature_for_function(function) else {
            return false;
        };
        !context
            .type_store
            .get(signature.return_type)
            .is_some_and(TyKind::is_unit)
    }

    fn signature_for_function(
        &self,
        function: &FunctionItem,
    ) -> Option<(DefId, &FunctionSignature)> {
        self.function_context?
            .signatures
            .functions
            .iter()
            .find_map(|(def_id, signature)| {
                (signature.span == function.span).then_some((*def_id, signature))
            })
    }

    fn check_block(&mut self, block: &Block) -> Flow {
        let mut exits = Flow::NONE;
        let mut falls_through = true;
        let mut terminator_span = None;
        let mut defers = Vec::new();
        for stmt in &block.stmts {
            if !falls_through {
                let summary = "unreachable statement";
                let mut diagnostic = Diagnostic::user_error(codes::STATIC_CHECK, summary)
                    .primary(stmt.span, "this statement cannot be reached");
                if let Some(terminator_span) = terminator_span {
                    diagnostic = diagnostic.secondary(
                        terminator_span,
                        "control flow terminates before reaching this statement",
                    );
                }
                self.diagnostics.push(diagnostic.finish());
                // Continue walking unreachable syntax so nested invalid loop
                // control and duplicate patterns are not hidden by the first
                // terminating statement.
                self.check_stmt(stmt);
                continue;
            }
            if let StmtKind::Defer(expr) = &stmt.kind {
                defers.push(self.check_defer(expr));
                continue;
            }
            let stmt_flow = self.check_stmt(stmt);
            exits = exits.union(stmt_flow.without_fallthrough().through_defers(&defers));
            falls_through = stmt_flow.falls_through;
            if !falls_through {
                terminator_span = Some(stmt.span);
            }
        }
        if let Some(tail) = block.tail.as_deref() {
            let tail_flow = self.check_expr_flow(tail);
            if falls_through {
                exits = exits.union(tail_flow.without_fallthrough().through_defers(&defers));
                falls_through = tail_flow.falls_through;
            }
        }
        if falls_through {
            exits = exits.union(Flow::NEXT.through_defers(&defers));
        }
        let flow = exits;
        self.block_flows.insert(std::ptr::from_ref(block), flow);
        flow
    }

    fn tail_expr_returns_on_all_paths(&self, expr: &Expr) -> bool {
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
            ExprKind::Match(matched) => self.match_tail_covers_all_paths(matched),
            _ => true,
        }
    }

    fn match_tail_covers_all_paths(&self, matched: &nia_ast::MatchExpr) -> bool {
        // Body checking owns typed exhaustiveness. Flow checking only needs to
        // validate that every arm which can be selected produces a value or
        // terminates; treating the syntactically visible arms as a complete
        // constructor universe here would produce unsound return acceptance.
        let mut all_arms_produce = !matched.arms.is_empty();
        for arm in &matched.arms {
            all_arms_produce &= self.match_tail_arm_produces_value(&arm.body);
        }
        all_arms_produce
    }

    fn match_tail_arm_produces_value(&self, body: &MatchArmBody) -> bool {
        match body {
            MatchArmBody::Expr(_) => true,
            MatchArmBody::Stmt(stmt) => self
                .stmt_flows
                .get(&std::ptr::from_ref(stmt))
                .is_some_and(|flow| !flow.falls_through),
            MatchArmBody::Block(block) => self.block_returns_on_all_paths(block),
        }
    }

    fn block_returns_on_all_paths(&self, block: &Block) -> bool {
        self.block_flows
            .get(&std::ptr::from_ref(block))
            .is_some_and(|flow| !flow.falls_through)
            || block
                .tail
                .as_deref()
                .is_some_and(|tail| self.tail_expr_returns_on_all_paths(tail))
    }

    fn check_stmt(&mut self, stmt: &Stmt) -> Flow {
        let flow = match &stmt.kind {
            StmtKind::Binding(binding) => binding
                .value
                .as_ref()
                .map_or(Flow::NEXT, |value| self.check_expr_flow(value)),
            StmtKind::Static(binding) => binding
                .value
                .as_ref()
                .map_or(Flow::NEXT, |value| self.check_expr_flow(value)),
            StmtKind::Expr(expr) => self.check_expr_flow(expr),
            StmtKind::Using(_) => Flow::NEXT,
            StmtKind::Defer(expr) => self.check_defer(expr),
            StmtKind::Return(value) => {
                // A return terminates its enclosing block, but evaluating its
                // value still traverses nested matches, defers, and control
                // expressions for diagnostics.
                value
                    .as_ref()
                    .map_or(Flow::NEXT, |value| self.check_expr_flow(value))
                    .then(Flow::RETURN)
            }
            StmtKind::Break => {
                if self.loop_depth == 0 {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::STATIC_CHECK,
                        stmt.span,
                        "`break` and `continue` can only appear inside loops",
                    ));
                }
                Flow::BREAK
            }
            StmtKind::Continue => {
                if self.loop_depth == 0 {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::STATIC_CHECK,
                        stmt.span,
                        "`break` and `continue` can only appear inside loops",
                    ));
                }
                Flow::CONTINUE
            }
            StmtKind::ForIn(for_stmt) => {
                let iter_flow = self.check_expr_flow(&for_stmt.iter);
                self.loop_depth += 1;
                // A syntax-only pass cannot prove iteration or the absence of
                // a break, so an entered loop conservatively permits exit. An
                // iterator expression that terminates never enters it.
                let body_flow = self.check_block(&for_stmt.body);
                self.loop_depth -= 1;
                iter_flow.then(Flow::NEXT.union(Flow {
                    returns: body_flow.returns,
                    ..Flow::NONE
                }))
            }
            StmtKind::While(while_stmt) => {
                let cond_flow = self.check_expr_flow(&while_stmt.cond);
                self.loop_depth += 1;
                let body_flow = self.check_block(&while_stmt.body);
                self.loop_depth -= 1;
                cond_flow.then(Flow::NEXT.union(Flow {
                    returns: body_flow.returns,
                    ..Flow::NONE
                }))
            }
            StmtKind::Loop(loop_stmt) => {
                self.loop_depth += 1;
                let body_flow = self.check_block(&loop_stmt.body);
                self.loop_depth -= 1;
                Flow {
                    falls_through: body_flow.breaks_loop,
                    returns: body_flow.returns,
                    ..Flow::NONE
                }
            }
        };
        self.stmt_flows.insert(std::ptr::from_ref(stmt), flow);
        flow
    }

    fn check_expr_flow(&mut self, expr: &Expr) -> Flow {
        match &expr.kind {
            ExprKind::Block(block) => self.check_block(block),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let cond_flow = self.check_expr_flow(cond);
                let then_flow = self.check_block(then_branch);
                let else_flow = else_branch
                    .as_deref()
                    .map_or(Flow::NEXT, |else_branch| self.check_expr_flow(else_branch));
                cond_flow.then(then_flow.union(else_flow))
            }
            ExprKind::IfPattern(if_pattern) => {
                let target_flow = self.check_expr_flow(&if_pattern.target);
                self.check_pattern_flow(&if_pattern.pattern);
                let then_flow = self.check_block(&if_pattern.then_branch);
                let else_flow = if_pattern
                    .else_branch
                    .as_deref()
                    .map_or(Flow::NEXT, |else_branch| self.check_expr_flow(else_branch));
                target_flow.then(then_flow.union(else_flow))
            }
            ExprKind::IfPatternChain(chain) => {
                let failure_flow = chain
                    .else_branch
                    .as_deref()
                    .map_or(Flow::NEXT, |else_branch| self.check_expr_flow(else_branch));
                let mut flow = Flow::NONE;
                let mut reaches_next_clause = true;
                for clause in &chain.clauses {
                    let clause_flow = match clause {
                        nia_ast::IfPatternChainClause::Pattern { target, pattern } => {
                            let target_flow = self.check_expr_flow(target);
                            self.check_pattern_flow(pattern);
                            target_flow
                        }
                        nia_ast::IfPatternChainClause::Condition(condition) => {
                            self.check_expr_flow(condition)
                        }
                    };
                    if reaches_next_clause {
                        flow = flow.union(clause_flow.without_fallthrough());
                        if clause_flow.falls_through {
                            flow = flow.union(failure_flow);
                        }
                        reaches_next_clause = clause_flow.falls_through;
                    }
                }
                let then_flow = self.check_block(&chain.then_branch);
                if reaches_next_clause {
                    flow = flow.union(then_flow);
                }
                flow
            }
            ExprKind::Match(matched) => {
                self.check_match_patterns(matched);
                let target_flow = self.check_expr_flow(&matched.target);
                let mut coverage = SyntacticPatternCoverage::default();
                let mut arms_flow = Flow::NONE;
                for arm in &matched.arms {
                    for pattern in &arm.patterns {
                        coverage.record(pattern);
                        self.check_pattern_flow(pattern);
                    }
                    let arm_flow = self.check_match_arm_flow(&arm.body);
                    arms_flow = arms_flow.union(arm_flow);
                }
                if !coverage.covers_all() {
                    arms_flow = arms_flow.union(Flow::NEXT);
                }
                target_flow.then(arms_flow)
            }
            ExprKind::BracketSuffix { callee, args } => {
                let mut flow = self.check_expr_flow(callee);
                for arg in args {
                    if let Some(expr) = &arg.expr {
                        flow = flow.then(self.check_expr_flow(expr));
                    }
                }
                flow
            }
            ExprKind::Tuple(elems) => {
                let mut flow = Flow::NEXT;
                for elem in elems {
                    flow = flow.then(self.check_expr_flow(elem));
                }
                flow
            }
            ExprKind::Closure { captures, body, .. } => {
                let mut flow = Flow::NEXT;
                for capture in captures {
                    flow = flow.then(self.check_expr_flow(&capture.value));
                }
                // A closure body is a separate function-like control-flow
                // region. Its `break`/`continue` cannot target a loop around
                // the closure expression, while loops declared inside the
                // closure are tracked normally by the recursive walk.
                let enclosing_loop_depth = self.loop_depth;
                self.loop_depth = 0;
                self.check_expr_flow(body);
                self.loop_depth = enclosing_loop_depth;
                flow
            }
            ExprKind::ArrayLiteral { elems } => {
                let mut flow = Flow::NEXT;
                match elems {
                    nia_ast::ArrayElements::List(elems) => {
                        for elem in elems {
                            flow = flow.then(self.check_expr_flow(elem));
                        }
                    }
                    nia_ast::ArrayElements::Repeat { value, count } => {
                        flow = flow.then(self.check_expr_flow(value));
                        flow = flow.then(self.check_expr_flow(count));
                    }
                }
                flow
            }
            ExprKind::TypedStructLiteral { fields, .. } => {
                let mut flow = Flow::NEXT;
                for field in fields {
                    flow = flow.then(self.check_expr_flow(&field.value));
                }
                flow
            }
            ExprKind::QualifiedStructLiteral { target, fields } => {
                let mut flow = self.check_expr_flow(target);
                for field in fields {
                    flow = flow.then(self.check_expr_flow(&field.value));
                }
                flow
            }
            ExprKind::OmittedAggregateLiteral { fields } => {
                let mut flow = Flow::NEXT;
                for field in fields {
                    flow = flow.then(self.check_expr_flow(&field.value));
                }
                flow
            }
            ExprKind::Unary { expr, .. }
            | ExprKind::OptionalSome { expr }
            | ExprKind::ErrorOk { expr }
            | ExprKind::ErrorErr { expr }
            | ExprKind::Try { expr }
            | ExprKind::Cast { expr, .. } => self.check_expr_flow(expr),
            ExprKind::Binary { lhs, op, rhs } => {
                let lhs_flow = self.check_expr_flow(lhs);
                let rhs_flow = self.check_expr_flow(rhs);
                // The RHS of logical operators is conditional, so its
                // termination cannot prove that the complete expression
                // terminates. Every other binary operand is unconditional.
                if matches!(op, nia_ast::BinaryOp::And | nia_ast::BinaryOp::Or) {
                    lhs_flow.then(Flow::NEXT.union(rhs_flow))
                } else {
                    lhs_flow.then(rhs_flow)
                }
            }
            ExprKind::Assign { lhs, rhs, .. } => {
                let rhs_flow = self.check_expr_flow(rhs);
                let lhs_flow = self.check_expr_flow(lhs);
                rhs_flow.then(lhs_flow)
            }
            ExprKind::Call { callee, args } => {
                let mut flow = self.check_expr_flow(callee);
                for arg in args {
                    flow = flow.then(self.check_expr_flow(arg));
                }
                flow
            }
            ExprKind::Field { lhs, .. } | ExprKind::TupleField { lhs, .. } => {
                self.check_expr_flow(lhs)
            }
            ExprKind::Index { lhs, index } => {
                let mut flow = self.check_expr_flow(lhs);
                match index {
                    IndexArg::Expr(index) => {
                        flow = flow.then(self.check_expr_flow(index));
                    }
                    IndexArg::Range(range) => {
                        if let Some(start) = &range.start {
                            flow = flow.then(self.check_expr_flow(start));
                        }
                        if let Some(end) = &range.end {
                            flow = flow.then(self.check_expr_flow(end));
                        }
                    }
                }
                flow
            }
            ExprKind::Range(range) => {
                let mut flow = Flow::NEXT;
                if let Some(start) = &range.start {
                    flow = flow.then(self.check_expr_flow(start));
                }
                if let Some(end) = &range.end {
                    flow = flow.then(self.check_expr_flow(end));
                }
                flow
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
            | ExprKind::Qualified { .. }
            | ExprKind::OmittedMember { .. } => Flow::NEXT,
        }
    }

    fn check_match_arm_flow(&mut self, body: &MatchArmBody) -> Flow {
        match body {
            MatchArmBody::Expr(expr) => self.check_expr_flow(expr),
            MatchArmBody::Stmt(stmt) => self.check_stmt(stmt),
            MatchArmBody::Block(block) => self.check_block(block),
        }
    }

    fn check_defer(&mut self, expr: &Expr) -> Flow {
        self.check_expr_flow(expr)
    }

    fn check_pattern_flow(&mut self, pattern: &Pattern) {
        match &pattern.kind {
            PatternKind::Wildcard | PatternKind::Bind { .. } | PatternKind::OptionalNull => {}
            PatternKind::Pointer(pattern)
            | PatternKind::MutPointer(pattern)
            | PatternKind::OptionalSome(pattern)
            | PatternKind::ErrorOk(pattern)
            | PatternKind::ErrorErr(pattern) => self.check_pattern_flow(pattern),
            PatternKind::Tuple(fields) => {
                for field in fields {
                    self.check_pattern_flow(field);
                }
            }
            PatternKind::Nominal {
                constructor: variant,
                fields,
            } => {
                self.check_expr_flow(variant);
                match fields {
                    nia_ast::NominalPatternFields::Tuple(fields) => {
                        for field in fields {
                            self.check_pattern_flow(field);
                        }
                    }
                    nia_ast::NominalPatternFields::Named { fields, .. } => {
                        for field in fields {
                            self.check_pattern_flow(&field.pattern);
                        }
                    }
                }
            }
            PatternKind::Expr(expr) => {
                self.check_expr_flow(expr);
            }
            PatternKind::Range { start, end, .. } => {
                self.check_expr_flow(start);
                self.check_expr_flow(end);
            }
        }
    }

    fn check_match_patterns(&mut self, matched: &nia_ast::MatchExpr) {
        let mut has_default = false;
        let mut seen = HashSet::new();
        for arm in &matched.arms {
            for pattern in &arm.patterns {
                if Self::pattern_is_catch_all(pattern) {
                    if has_default {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::STATIC_CHECK,
                            arm.span,
                            "duplicate match default",
                        ));
                    }
                    has_default = true;
                    continue;
                }
                if let Some(fingerprint) = Self::pattern_fingerprint(pattern)
                    && !seen.insert(fingerprint)
                {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::STATIC_CHECK,
                        pattern.span,
                        "duplicate match pattern",
                    ));
                }
            }
        }
    }

    fn pattern_is_catch_all(pattern: &Pattern) -> bool {
        match &pattern.kind {
            PatternKind::Wildcard | PatternKind::Bind { .. } => true,
            PatternKind::Pointer(inner) | PatternKind::MutPointer(inner) => {
                Self::pattern_is_catch_all(inner)
            }
            PatternKind::Tuple(fields) => fields.iter().all(Self::pattern_is_catch_all),
            PatternKind::OptionalSome(_)
            | PatternKind::OptionalNull
            | PatternKind::ErrorOk(_)
            | PatternKind::ErrorErr(_)
            | PatternKind::Nominal { .. }
            | PatternKind::Expr(_)
            | PatternKind::Range { .. } => false,
        }
    }

    fn pattern_fingerprint(pattern: &Pattern) -> Option<PatternFingerprint> {
        match &pattern.kind {
            PatternKind::Wildcard | PatternKind::Bind { .. } => None,
            PatternKind::Pointer(inner) => Some(PatternFingerprint::Pointer(Box::new(
                Self::pattern_fingerprint(inner)?,
            ))),
            PatternKind::MutPointer(inner) => Some(PatternFingerprint::MutPointer(Box::new(
                Self::pattern_fingerprint(inner)?,
            ))),
            PatternKind::OptionalSome(inner) => Some(PatternFingerprint::OptionalSome(Box::new(
                Self::pattern_fingerprint(inner)?,
            ))),
            PatternKind::OptionalNull => Some(PatternFingerprint::OptionalNull),
            PatternKind::ErrorOk(inner) => Some(PatternFingerprint::ErrorOk(Box::new(
                Self::pattern_fingerprint(inner)?,
            ))),
            PatternKind::ErrorErr(inner) => Some(PatternFingerprint::ErrorErr(Box::new(
                Self::pattern_fingerprint(inner)?,
            ))),
            PatternKind::Tuple(fields) => Some(PatternFingerprint::Tuple(
                fields
                    .iter()
                    .map(Self::pattern_fingerprint)
                    .collect::<Option<Vec<_>>>()?,
            )),
            PatternKind::Nominal {
                constructor: variant,
                fields,
            } => {
                let fields = match fields {
                    nia_ast::NominalPatternFields::Tuple(fields) => fields
                        .iter()
                        .map(|field| Some((None, Self::pattern_fingerprint(field)?)))
                        .collect::<Option<Vec<_>>>()?,
                    nia_ast::NominalPatternFields::Named { fields, .. } => fields
                        .iter()
                        .map(|field| {
                            Some((Some(field.name), Self::pattern_fingerprint(&field.pattern)?))
                        })
                        .collect::<Option<Vec<_>>>()?,
                };
                Some(PatternFingerprint::EnumVariant {
                    variant: Self::expr_fingerprint(variant)?,
                    fields,
                })
            }
            PatternKind::Expr(expr) => {
                Some(PatternFingerprint::Expr(Self::expr_fingerprint(expr)?))
            }
            PatternKind::Range {
                start,
                end,
                inclusive,
            } => Some(PatternFingerprint::Range {
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
            ExprKind::Qualified { lhs, name, .. } => Some(ExprFingerprint::Qualified(
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
    use nia_defs::collect_module_defs;
    use nia_ids::ModuleIdAllocator;
    use nia_item_signatures::{ItemSignatureInput, ItemSignatureSource, collect_item_signatures};
    use nia_parser::parse_module;
    use nia_type_lower::{TypeLowering, TypeLoweringContext};
    use nia_type_resolve::resolve_module_types;

    fn lower_module_types_with_context(
        module_id: ModuleId,
        module: &nia_ast::Module,
        resolved: &nia_type_resolve::TypeResolution,
        context: TypeLoweringContext<'_>,
    ) -> TypeLowering {
        nia_type_lower::lower_module_types_with_context(module_id, module, resolved, context)
            .expect("lower test types")
    }

    fn pipeline(source: &str) -> FlowCheck {
        let (module, parse_errors) = parse_module(source);
        assert!(parse_errors.is_empty(), "{parse_errors:?}");
        let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
        let module_id = module_ids.allocate().expect("allocate module ID");
        let defs = collect_module_defs(module_id, &module).expect("collect definitions");
        let resolved = resolve_module_types(&module, &defs);
        let type_store = TypeStore::new().expect("create type store");
        let lowered = lower_module_types_with_context(
            module_id,
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
        })
        .expect("collect test signatures");
        check_module_flow(&module, &type_store, &signatures)
    }

    fn pipeline_with_reachable_filter(source: &str) -> FlowCheck {
        let (module, parse_errors) = parse_module(source);
        assert!(parse_errors.is_empty(), "{parse_errors:?}");
        let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
        let module_id = module_ids.allocate().expect("allocate module ID");
        let defs = collect_module_defs(module_id, &module).expect("collect definitions");
        let resolved = resolve_module_types(&module, &defs);
        let type_store = TypeStore::new().expect("create type store");
        let lowered = lower_module_types_with_context(
            module_id,
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
        })
        .expect("collect test signatures");
        let main = defs
            .module_scope
            .values
            .get(&nia_symbol::known::MAIN)
            .expect("test source must define main");
        let item_tree = ModuleItemTree::from_module(&module);
        let active_item_tree = item_tree.all_items_active();
        let reachable = HashSet::from([GlobalDefId {
            module_id,
            def_id: main,
        }]);
        check_active_module_flow_with_signatures_and_filter(
            &active_item_tree,
            &type_store,
            FlowCheckSignatures {
                functions: &signatures.functions,
            },
            FlowCheckFilter::ReachableFunctions {
                module_id,
                functions: &reachable,
            },
        )
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
        let missing_return = checked
            .diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic
                    .summary
                    .contains("does not return on all reachable paths")
            })
            .expect("missing return diagnostic");
        assert!(missing_return.labels.iter().any(|label| {
            label.style == nia_diagnostic::LabelStyle::Secondary
                && label
                    .message
                    .as_deref()
                    .is_some_and(|message| message.contains("function declaration"))
        }));
        assert!(
            missing_return
                .help
                .iter()
                .any(|help| help.contains("change the function return type to `()`"))
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("unreachable statement"))
        );
        let unreachable = checked
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.summary.contains("unreachable statement"))
            .expect("unreachable statement diagnostic");
        assert!(unreachable.labels.iter().any(|label| {
            label.style == nia_diagnostic::LabelStyle::Secondary
                && label
                    .message
                    .as_deref()
                    .is_some_and(|message| message.contains("terminates before"))
        }));
    }

    #[test]
    fn non_breaking_loop_terminates_enclosing_control_flow() {
        let checked = pipeline(
            r#"
fn spin() i32 {
    loop {}
}

fn returnsFromLoop() i32 {
    loop {
        return 1;
    }
}

fn unreachableAfterLoop() {
    loop {
        continue;
    }
    let value = 1;
}
"#,
        );
        assert!(
            checked.diagnostics.iter().all(|diagnostic| !diagnostic
                .summary
                .contains("does not return on all reachable paths")),
            "non-breaking loops must not fall through: {:?}",
            checked.diagnostics
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("unreachable statement")),
            "a statement after a non-breaking loop must be unreachable: {:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn reachable_break_allows_loop_to_fall_through() {
        let checked = pipeline(
            r#"
fn maybeExit(flag: bool) i32 {
    loop {
        if flag {
            break;
        }
    }
}

fn nestedBreakDoesNotExitOuter() i32 {
    loop {
        loop {
            break;
        }
    }
}

fn unreachableBreakDoesNotExitLoop() i32 {
    loop {
        continue;
        break;
    }
}
"#,
        );
        assert_eq!(
            checked
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic
                    .summary
                    .contains("does not return on all reachable paths"))
                .count(),
            1,
            "only the loop with a reachable break may fall through: {:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn deferred_control_flow_obeys_registration_and_lifo_override() {
        let checked = pipeline(
            r#"
fn deferredBreak() i32 {
    loop {
        defer {
            break;
        };
        continue;
    }
}

fn deferredReturn() i32 {
    loop {
        defer {
            return 1;
        };
        continue;
    }
}

fn outerBreakOverridesInnerReturn() i32 {
    loop {
        defer {
            break;
        };
        defer {
            return 1;
        };
        continue;
    }
}

fn outerReturnOverridesInnerBreak() i32 {
    loop {
        defer {
            return 1;
        };
        defer {
            break;
        };
        continue;
    }
}
"#,
        );
        assert_eq!(
            checked
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic
                    .summary
                    .contains("does not return on all reachable paths"))
                .count(),
            2,
            "deferred exits must run in LIFO order and override the triggering exit: {:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn checks_match_duplicate_default_and_patterns() {
        let checked = pipeline(
            r#"
fn main(x: i32) {
    match x {
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
                .any(|diagnostic| diagnostic.summary.contains("duplicate match pattern"))
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("duplicate match default"))
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
    fn exhaustive_match_returns_on_all_paths() {
        let checked = pipeline(
            r#"
fn name(x: u32) &u8 {
    match x {
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
    fn destructuring_match_returns_on_all_paths() {
        let checked = pipeline(
            r#"
fn optional(value: ?i32) i32 {
    match value {
        ?payload => return payload,
        null => return 0,
    }
}

fn nested(value: ?(i32!i32)) i32 {
    match value {
        ?!payload => return payload,
        ?error! => return error,
        null => return 0,
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
    fn match_tail_expression_satisfies_return_analysis() {
        let checked = pipeline(
            r#"
fn name(x: u32) &u8 {
    match x {
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
    fn exhaustive_match_tail_without_default_satisfies_return_analysis() {
        let checked = pipeline(
            r#"
enum Mode: u8 {
    A,
    B,
}

fn name(mode: Mode) u32 {
    match mode {
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
        match 1 {
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
        match 1 {
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
            2,
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
    fn reports_unreachable_after_match_arm_control_flow_blocks() {
        let checked = pipeline(
            r#"
fn main(kind: i32) {
    match kind {
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

    #[test]
    fn checks_return_value_flow_before_terminating() {
        let checked = pipeline(
            r#"
fn main() i32 {
    return match 1 {
        1 => 1,
        1 => 2,
    };
}
"#,
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("duplicate match pattern")),
            "return expressions must retain nested flow diagnostics: {:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn closure_control_flow_does_not_inherit_enclosing_loop() {
        let checked = pipeline(
            r#"
fn main() () {
    loop {
        let callback = \value: i32 -> {
            break;
        };
        break;
    }
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
            "closure break must not target an enclosing loop: {:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn propagates_termination_from_eager_expression_operands() {
        let checked = pipeline(
            r#"
fn consume(value: i32) {}
fn truth(value: i32) bool { true }

fn call_arg() i32 {
    consume({ return 1; });
}

fn unary_operand() i32 {
    -{ return 1; };
}

fn binary_operand() i32 {
    1 + { return 1; };
}

fn condition() i32 {
    if truth({ return 1; }) {
        return 2;
    };
}

fn while_condition() i32 {
    while truth({ return 1; }) {}
}
"#,
        );
        assert!(
            checked.diagnostics.iter().all(|diagnostic| !diagnostic
                .summary
                .contains("does not return on all reachable paths")),
            "eager operand termination must reach the enclosing statement: {:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn short_circuit_rhs_does_not_prove_enclosing_termination() {
        let checked = pipeline(
            r#"
fn short_circuit(flag: bool) i32 {
    flag and { return 1; true };
}
"#,
        );
        assert!(
            checked.diagnostics.iter().any(|diagnostic| diagnostic
                .summary
                .contains("does not return on all reachable paths")),
            "a skipped logical RHS leaves a fallthrough path: {:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn reachable_filter_uses_stable_function_identity() {
        let checked = pipeline_with_reachable_filter(
            r#"
fn main(flag: bool) i32 {
    if flag {
        return 1;
    }
}

fn helper(flag: bool) i32 {
    if flag {
        return 2;
    }
    match 1 {
        1 => return 3,
        1 => return 4,
    }
}
"#,
        );
        assert_eq!(
            checked
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic
                    .summary
                    .contains("does not return on all reachable paths"))
                .count(),
            1,
            "only reachable main should receive missing-return analysis: {:?}",
            checked.diagnostics
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .all(|diagnostic| !diagnostic.summary.contains("duplicate match pattern")),
            "filtered helper syntax must not leak diagnostics: {:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn traverses_function_tail_expression_diagnostics_once() {
        let checked = pipeline(
            r#"
fn consume(value: i32) i32 { value }

fn tail_match() i32 {
    consume(match 1 {
        1 => 1,
        1 => 2,
    })
}

fn tail_closure() () {
    \ -> {
        break;
    }
}
"#,
        );
        assert_eq!(
            checked
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.summary.contains("duplicate match pattern"))
                .count(),
            1,
            "tail matches must be traversed without duplicate diagnostics: {:?}",
            checked.diagnostics
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
            "tail closure bodies must be checked as independent flow regions: {:?}",
            checked.diagnostics
        );
    }
}
