// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{
    Block, Expr, ExprKind, FunctionItem, IndexArg, MatchArmBody, Module, Pattern, PatternKind,
    Stmt, StmtKind,
};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{DefId, GlobalDefId, ModuleId};
use nia_item_signatures::{FunctionSignature, ItemSignatures};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNode, ItemTreeNodeKind, ModuleItemTree};
use nia_symbol::SymbolId;
use nia_ty::{TyKind, TypeStore};
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
                "non-unit function does not return on all reachable paths",
            ));
        }
    }

    fn function_requires_return(&self, function: &FunctionItem) -> bool {
        let Some((_, signature)) = self.signature_for_function(function) else {
            return false;
        };
        !self
            .type_store
            .get(signature.return_type)
            .is_some_and(TyKind::is_unit)
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
            ExprKind::Match(matched) => self.match_tail_covers_all_paths(matched),
            _ => true,
        }
    }

    fn match_tail_covers_all_paths(&mut self, matched: &nia_ast::MatchExpr) -> bool {
        self.check_match_patterns(matched);
        let mut all_arms_produce = !matched.arms.is_empty();
        for arm in &matched.arms {
            all_arms_produce &= self.match_tail_arm_produces_value(&arm.body);
        }
        all_arms_produce
    }

    fn match_tail_arm_produces_value(&mut self, body: &MatchArmBody) -> bool {
        match body {
            MatchArmBody::Expr(_) => true,
            MatchArmBody::Stmt(stmt) => !self.check_stmt(stmt).falls_through,
            MatchArmBody::Block(block) => self.block_returns_on_all_paths(block),
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
                self.check_pattern_flow(&if_pattern.pattern);
                let then_falls_through = self.check_block(&if_pattern.then_branch).falls_through;
                let mut falls_through = if_pattern.else_branch.is_none() || then_falls_through;
                if let Some(else_branch) = &if_pattern.else_branch {
                    falls_through |= self.check_expr_flow(else_branch).falls_through;
                }
                Flow { falls_through }
            }
            ExprKind::Match(matched) => {
                self.check_match_patterns(matched);
                self.check_expr_flow(&matched.target);
                let mut coverage = SyntacticPatternCoverage::default();
                let mut all_arms_terminate = !matched.arms.is_empty();
                for arm in &matched.arms {
                    for pattern in &arm.patterns {
                        coverage.record(pattern);
                        self.check_pattern_flow(pattern);
                    }
                    all_arms_terminate &= !self.check_match_arm_flow(&arm.body).falls_through;
                }
                Flow {
                    falls_through: !(coverage.covers_all() && all_arms_terminate),
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
            ExprKind::Tuple(elems) => {
                for elem in elems {
                    self.check_expr_flow(elem);
                }
                Flow {
                    falls_through: true,
                }
            }
            ExprKind::Closure { captures, body, .. } => {
                for capture in captures {
                    self.check_expr_flow(&capture.value);
                }
                self.check_expr_flow(body);
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
            ExprKind::TypedStructLiteral { fields, .. } => {
                for field in fields {
                    self.check_expr_flow(&field.value);
                }
                Flow {
                    falls_through: true,
                }
            }
            ExprKind::QualifiedStructLiteral { target, fields } => {
                self.check_expr_flow(target);
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
            ExprKind::TupleField { lhs, .. } => {
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

    fn check_match_arm_flow(&mut self, body: &MatchArmBody) -> Flow {
        match body {
            MatchArmBody::Expr(expr) => self.check_expr_flow(expr),
            MatchArmBody::Stmt(stmt) => self.check_stmt(stmt),
            MatchArmBody::Block(block) => self.check_block(block),
        }
    }

    fn check_defer(&mut self, expr: &Expr) {
        self.check_expr_flow(expr);
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
    use nia_defs::collect_module_defs;
    use nia_ids::ModuleIdAllocator;
    use nia_item_signatures::{ItemSignatureInput, ItemSignatureSource, collect_item_signatures};
    use nia_parser::parse_module;
    use nia_type_lower::{TypeLoweringContext, lower_module_types_with_context};
    use nia_type_resolve::resolve_module_types;

    fn pipeline(source: &str) -> FlowCheck {
        let (module, parse_errors) = parse_module(source);
        assert!(parse_errors.is_empty(), "{parse_errors:?}");
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let defs = collect_module_defs(module_id, &module);
        let resolved = resolve_module_types(&module, &defs);
        let type_store = TypeStore::new();
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
}
