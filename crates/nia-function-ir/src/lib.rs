// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{AssignOp, BinaryOp, UnaryOp};
use nia_body_ir::{
    AsmOption, BuiltinConst, PlaceBase, PlaceElem, TypedArrayElements, TypedBinding, TypedBody,
    TypedCallee, TypedExpr, TypedExprKind, TypedForHeader, TypedForInit, TypedInlineAsm,
    TypedLocal, TypedLocalKind, TypedPlace, TypedSliceRange, TypedStmt, TypedStmtKind, TypedSwitch,
    TypedSwitchArmBody, TypedSwitchPattern,
};
use nia_ids::{InternedTyId, LocalId};
use nia_span::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionBlockId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionScopeId(pub u32);

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionBody {
    pub span: Span,
    pub locals: Vec<TypedLocal>,
    pub scopes: Vec<FunctionScope>,
    pub blocks: Vec<FunctionBlock>,
    pub entry: FunctionBlockId,
    pub ty: InternedTyId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionScope {
    pub id: FunctionScopeId,
    pub parent: Option<FunctionScopeId>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionBlock {
    pub id: FunctionBlockId,
    pub scope: FunctionScopeId,
    pub span: Span,
    pub ops: Vec<FunctionOp>,
    pub terminator: FunctionTerminator,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionOp {
    Binding(FunctionBinding),
    StoreLocal {
        local_id: LocalId,
        value: FunctionExpr,
        span: Span,
    },
    Expr(FunctionExpr),
    Defer(FunctionDeferBody),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionBinding {
    pub local_id: LocalId,
    pub name: String,
    pub ty: InternedTyId,
    pub value: Option<FunctionExpr>,
    pub is_const: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDeferBody {
    pub span: Span,
    pub scopes: Vec<FunctionScope>,
    pub blocks: Vec<FunctionBlock>,
    pub entry: FunctionBlockId,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionTerminator {
    Branch {
        target: FunctionBlockId,
        span: Span,
    },
    Next {
        target: FunctionBlockId,
        span: Span,
    },
    If {
        cond: FunctionExpr,
        then_target: FunctionBlockId,
        else_target: FunctionBlockId,
        span: Span,
    },
    Switch {
        target: FunctionExpr,
        arms: Vec<FunctionSwitchArm>,
        default: Option<FunctionBlockId>,
        fallback: FunctionBlockId,
        span: Span,
    },
    Loop {
        header: FunctionForHeader,
        body: FunctionBlockId,
        continue_target: FunctionBlockId,
        break_target: FunctionBlockId,
        span: Span,
    },
    Return {
        value: Option<FunctionExpr>,
        span: Span,
    },
    Tail {
        value: Option<FunctionExpr>,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionSwitchArm {
    pub pattern: FunctionExpr,
    pub target: FunctionBlockId,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionForHeader {
    Infinite,
    Condition(FunctionExpr),
    CStyle { cond: Option<Box<FunctionExpr>> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionExpr {
    pub span: Span,
    pub ty: InternedTyId,
    pub kind: FunctionExprKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionExprKind {
    Error,
    Integer(String),
    Float(String),
    String(Vec<u32>),
    ByteString(Vec<u8>),
    Char(u32),
    ByteChar(String),
    Bool(bool),
    Local(LocalId),
    Global(nia_ids::GlobalDefId),
    Function(nia_ids::GlobalDefId),
    FunctionInstance {
        def_id: nia_ids::GlobalDefId,
        args: Vec<InternedTyId>,
    },
    EnumVariant(nia_ids::GlobalDefId),
    BuiltinValue(BuiltinConst),
    Len(Box<FunctionExpr>),
    Ptr(Box<FunctionExpr>),
    InlineAsm(FunctionInlineAsm),
    CStringPointer {
        array: Box<FunctionExpr>,
        is_const: bool,
    },
    ArrayLiteral {
        elems: FunctionArrayElements,
    },
    StructLiteral {
        def_id: nia_ids::GlobalDefId,
        fields: Vec<FunctionFieldInit>,
    },
    UnionLiteral {
        def_id: nia_ids::GlobalDefId,
        field: Box<FunctionFieldInit>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<FunctionExpr>,
    },
    Binary {
        lhs: Box<FunctionExpr>,
        op: BinaryOp,
        rhs: Box<FunctionExpr>,
    },
    Assign {
        place: FunctionPlace,
        op: AssignOp,
        rhs: Box<FunctionExpr>,
    },
    Discard(Box<FunctionExpr>),
    Cast {
        expr: Box<FunctionExpr>,
        ty: InternedTyId,
    },
    Call {
        callee: FunctionCallee,
        args: Vec<FunctionExpr>,
    },
    Field {
        lhs: Box<FunctionExpr>,
        field: nia_ids::GlobalDefId,
    },
    Index {
        lhs: Box<FunctionExpr>,
        index: Box<FunctionExpr>,
    },
    Slice {
        lhs: Box<FunctionExpr>,
        range: FunctionSliceRange,
        is_const: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionSliceRange {
    pub start: Option<Box<FunctionExpr>>,
    pub end: Option<Box<FunctionExpr>>,
    pub inclusive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionInlineAsm {
    pub code: String,
    pub inputs: Vec<FunctionAsmInput>,
    pub outputs: Vec<FunctionAsmOutput>,
    pub clobbers: Vec<String>,
    pub options: Vec<AsmOption>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionAsmInput {
    pub constraint: String,
    pub value: FunctionExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionAsmOutput {
    pub constraint: String,
    pub place: FunctionPlace,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionArrayElements {
    List(Vec<FunctionExpr>),
    Repeat {
        value: Box<FunctionExpr>,
        count: u64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionFieldInit {
    pub field: nia_ids::GlobalDefId,
    pub name: String,
    pub value: FunctionExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionCallee {
    Function(nia_ids::GlobalDefId),
    FunctionInstance {
        def_id: nia_ids::GlobalDefId,
        args: Vec<InternedTyId>,
    },
    Method {
        def_id: nia_ids::GlobalDefId,
        args: Vec<InternedTyId>,
        receiver: Box<FunctionExpr>,
    },
    FunctionPointer(Box<FunctionExpr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionPlace {
    pub span: Span,
    pub ty: InternedTyId,
    pub base: FunctionPlaceBase,
    pub elems: Vec<FunctionPlaceElem>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionPlaceBase {
    Local(LocalId),
    Global(nia_ids::GlobalDefId),
    Deref(Box<FunctionExpr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionPlaceElem {
    Field(nia_ids::GlobalDefId),
    Index(Box<FunctionExpr>),
}

impl FunctionTerminator {
    pub fn successors(&self) -> Vec<FunctionBlockId> {
        match self {
            FunctionTerminator::Branch { target, .. } | FunctionTerminator::Next { target, .. } => {
                vec![*target]
            }
            FunctionTerminator::If {
                then_target,
                else_target,
                ..
            } => vec![*then_target, *else_target],
            FunctionTerminator::Switch {
                arms,
                default,
                fallback,
                ..
            } => arms
                .iter()
                .map(|arm| arm.target)
                .chain(default.or(Some(*fallback)))
                .collect(),
            FunctionTerminator::Loop {
                body, break_target, ..
            } => vec![*body, *break_target],
            FunctionTerminator::Return { .. } | FunctionTerminator::Tail { .. } => Vec::new(),
        }
    }
}

impl FunctionBody {
    pub fn block(&self, id: FunctionBlockId) -> Option<&FunctionBlock> {
        self.blocks.iter().find(|block| block.id == id)
    }

    pub fn scope(&self, id: FunctionScopeId) -> Option<&FunctionScope> {
        self.scopes.iter().find(|scope| scope.id == id)
    }

    pub fn edge_exited_scopes(
        &self,
        from: FunctionBlockId,
        to: FunctionBlockId,
    ) -> Option<Vec<FunctionScopeId>> {
        let from = self.block(from)?.scope;
        let to = self.block(to)?.scope;
        self.exited_scopes_between(from, Some(to))
    }

    pub fn return_exited_scopes(&self, from: FunctionBlockId) -> Option<Vec<FunctionScopeId>> {
        let from = self.block(from)?.scope;
        self.exited_scopes_between(from, None)
    }

    pub fn exited_scopes_between(
        &self,
        from: FunctionScopeId,
        to: Option<FunctionScopeId>,
    ) -> Option<Vec<FunctionScopeId>> {
        let from_chain = self.scope_chain_to_root(from)?;
        let to_chain = match to {
            Some(scope) => self.scope_chain_to_root(scope)?,
            None => Vec::new(),
        };
        let lca = from_chain
            .iter()
            .find(|scope| to_chain.contains(scope))
            .copied();
        Some(
            from_chain
                .into_iter()
                .take_while(|scope| Some(*scope) != lca)
                .collect(),
        )
    }

    fn scope_chain_to_root(&self, scope: FunctionScopeId) -> Option<Vec<FunctionScopeId>> {
        let mut chain = Vec::new();
        let mut current = Some(scope);
        while let Some(scope) = current {
            chain.push(scope);
            current = self.scope(scope)?.parent;
        }
        Some(chain)
    }
}

impl FunctionDeferBody {
    pub fn block(&self, id: FunctionBlockId) -> Option<&FunctionBlock> {
        self.blocks.iter().find(|block| block.id == id)
    }

    pub fn scope(&self, id: FunctionScopeId) -> Option<&FunctionScope> {
        self.scopes.iter().find(|scope| scope.id == id)
    }

    pub fn edge_exited_scopes(
        &self,
        from: FunctionBlockId,
        to: FunctionBlockId,
    ) -> Option<Vec<FunctionScopeId>> {
        let from = self.block(from)?.scope;
        let to = self.block(to)?.scope;
        self.exited_scopes_between(from, Some(to))
    }

    pub fn return_exited_scopes(&self, from: FunctionBlockId) -> Option<Vec<FunctionScopeId>> {
        let from = self.block(from)?.scope;
        self.exited_scopes_between(from, None)
    }

    pub fn exited_scopes_between(
        &self,
        from: FunctionScopeId,
        to: Option<FunctionScopeId>,
    ) -> Option<Vec<FunctionScopeId>> {
        let from_chain = self.scope_chain_to_root(from)?;
        let to_chain = match to {
            Some(scope) => self.scope_chain_to_root(scope)?,
            None => Vec::new(),
        };
        let lca = from_chain
            .iter()
            .find(|scope| to_chain.contains(scope))
            .copied();
        Some(
            from_chain
                .into_iter()
                .take_while(|scope| Some(*scope) != lca)
                .collect(),
        )
    }

    fn scope_chain_to_root(&self, scope: FunctionScopeId) -> Option<Vec<FunctionScopeId>> {
        let mut chain = Vec::new();
        let mut current = Some(scope);
        while let Some(scope) = current {
            chain.push(scope);
            current = self.scope(scope)?.parent;
        }
        Some(chain)
    }
}

pub fn lower_function_body(body: &TypedBody) -> FunctionBody {
    FunctionLowerer::new().lower_body(body)
}

struct FunctionLowerer {
    next_block: u32,
    next_scope: u32,
    next_temp_local: u32,
    temp_locals: Vec<TypedLocal>,
    scopes: Vec<FunctionScope>,
    loop_targets: Vec<LoopTargetIds>,
}

#[derive(Debug, Clone, Copy)]
struct LoopTargetIds {
    break_target: FunctionBlockId,
    continue_target: FunctionBlockId,
}

#[derive(Debug, Clone, Copy)]
enum Fallthrough {
    Tail,
    Branch(FunctionBlockId),
    StoreThenBranch {
        local_id: LocalId,
        target: FunctionBlockId,
    },
}

impl FunctionLowerer {
    fn new() -> Self {
        Self {
            next_block: 0,
            next_scope: 0,
            next_temp_local: 0,
            temp_locals: Vec::new(),
            scopes: Vec::new(),
            loop_targets: Vec::new(),
        }
    }

    fn lower_body(&mut self, body: &TypedBody) -> FunctionBody {
        self.next_temp_local = self.next_available_local(body);
        let root_scope = self.alloc_scope(None, body.span);
        let entry = self.alloc_block();
        let mut blocks = Vec::new();
        let mut locals = Vec::new();
        self.lower_body_into(body, entry, root_scope, &mut blocks, Fallthrough::Tail);
        self.collect_body_locals(body, &mut locals);
        locals.extend(self.temp_locals.clone());
        FunctionBody {
            span: body.span,
            locals,
            scopes: self.scopes.clone(),
            blocks,
            entry,
            ty: body.ty,
        }
    }

    fn lower_body_into(
        &mut self,
        body: &TypedBody,
        entry: FunctionBlockId,
        scope: FunctionScopeId,
        blocks: &mut Vec<FunctionBlock>,
        fallthrough: Fallthrough,
    ) {
        let mut current = entry;
        let mut ops = Vec::new();
        for stmt in &body.stmts {
            if self.lower_stmt_into(stmt, scope, &mut current, &mut ops, blocks) {
                return;
            }
        }
        self.finish_fallthrough_block(blocks, current, scope, body, ops, fallthrough);
    }

    fn lower_stmt_into(
        &mut self,
        stmt: &TypedStmt,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> bool {
        match &stmt.kind {
            TypedStmtKind::Binding(binding) => {
                let binding = self.lower_binding(binding, scope, current, ops, blocks);
                ops.push(FunctionOp::Binding(binding));
            }
            TypedStmtKind::Expr(expr) => {
                self.lower_expr_stmt(stmt.span, expr, scope, current, ops, blocks);
            }
            TypedStmtKind::Defer(expr) => {
                ops.push(FunctionOp::Defer(self.lower_defer_expr(expr)));
            }
            TypedStmtKind::Return(value) => {
                let value = value
                    .as_ref()
                    .map(|value| self.lower_value_expr(value, scope, current, ops, blocks));
                self.finish_block(
                    blocks,
                    *current,
                    scope,
                    stmt.span,
                    std::mem::take(ops),
                    FunctionTerminator::Return {
                        value,
                        span: stmt.span,
                    },
                );
                return true;
            }
            TypedStmtKind::Break => {
                let target = self
                    .loop_targets
                    .last()
                    .map(|targets| targets.break_target)
                    .unwrap_or(FunctionBlockId(u32::MAX));
                self.finish_block(
                    blocks,
                    *current,
                    scope,
                    stmt.span,
                    std::mem::take(ops),
                    FunctionTerminator::Branch {
                        target,
                        span: stmt.span,
                    },
                );
                return true;
            }
            TypedStmtKind::Continue => {
                let target = self
                    .loop_targets
                    .last()
                    .map(|targets| targets.continue_target)
                    .unwrap_or(FunctionBlockId(u32::MAX));
                self.finish_block(
                    blocks,
                    *current,
                    scope,
                    stmt.span,
                    std::mem::take(ops),
                    FunctionTerminator::Branch {
                        target,
                        span: stmt.span,
                    },
                );
                return true;
            }
            TypedStmtKind::For(for_stmt) => {
                self.lower_for_stmt(stmt.span, for_stmt, scope, current, ops, blocks);
            }
        }
        false
    }

    fn lower_expr_stmt(
        &mut self,
        span: Span,
        expr: &TypedExpr,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) {
        match &expr.kind {
            TypedExprKind::Block(body) => {
                self.lower_block_expr_stmt(span, body, scope, current, ops, blocks);
            }
            TypedExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.lower_if_expr_stmt(
                    span,
                    cond,
                    then_branch,
                    else_branch.as_deref(),
                    scope,
                    current,
                    ops,
                    blocks,
                );
            }
            TypedExprKind::Switch(switch) => {
                self.lower_switch_expr_stmt(span, switch, scope, current, ops, blocks);
            }
            _ => {
                let expr = self.lower_value_expr(expr, scope, current, ops, blocks);
                ops.push(FunctionOp::Expr(expr));
            }
        }
    }

    fn lower_block_expr_stmt(
        &mut self,
        span: Span,
        body: &TypedBody,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) {
        let body_entry = self.alloc_block();
        let after_block = self.alloc_block();
        self.finish_block(
            blocks,
            *current,
            scope,
            span,
            std::mem::take(ops),
            FunctionTerminator::Next {
                target: body_entry,
                span,
            },
        );
        let body_scope = self.alloc_scope(Some(scope), body.span);
        self.lower_body_into(
            body,
            body_entry,
            body_scope,
            blocks,
            Fallthrough::Branch(after_block),
        );
        *current = after_block;
    }

    fn lower_if_expr_stmt(
        &mut self,
        span: Span,
        cond: &TypedExpr,
        then_branch: &TypedBody,
        else_branch: Option<&TypedExpr>,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) {
        let cond = self.lower_value_expr(cond, scope, current, ops, blocks);
        let then_target = self.alloc_block();
        let else_target = else_branch.map(|_| self.alloc_block());
        let merge_target = self.alloc_block();
        self.finish_block(
            blocks,
            *current,
            scope,
            span,
            std::mem::take(ops),
            FunctionTerminator::If {
                cond,
                then_target,
                else_target: else_target.unwrap_or(merge_target),
                span,
            },
        );

        let then_scope = self.alloc_scope(Some(scope), then_branch.span);
        self.lower_body_into(
            then_branch,
            then_target,
            then_scope,
            blocks,
            Fallthrough::Branch(merge_target),
        );

        if let (Some(else_branch), Some(else_target)) = (else_branch, else_target) {
            let mut else_current = else_target;
            let mut else_ops = Vec::new();
            self.lower_expr_stmt(
                else_branch.span,
                else_branch,
                scope,
                &mut else_current,
                &mut else_ops,
                blocks,
            );
            self.finish_block(
                blocks,
                else_current,
                scope,
                else_branch.span,
                else_ops,
                FunctionTerminator::Branch {
                    target: merge_target,
                    span: else_branch.span,
                },
            );
        }

        *current = merge_target;
    }

    fn lower_switch_expr_stmt(
        &mut self,
        span: Span,
        switch: &TypedSwitch,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) {
        let target = self.lower_value_expr(&switch.target, scope, current, ops, blocks);
        let merge_target = self.alloc_block();
        let mut arms = Vec::new();
        let mut lowered_arms = Vec::new();
        let mut default = None;
        for arm in &switch.arms {
            let arm_target = self.alloc_block();
            match &arm.pattern {
                TypedSwitchPattern::Expr(pattern) => arms.push(FunctionSwitchArm {
                    pattern: self.lower_value_expr(pattern, scope, current, ops, blocks),
                    target: arm_target,
                }),
                TypedSwitchPattern::Default => default = Some(arm_target),
            }
            lowered_arms.push((arm_target, arm));
        }

        self.finish_block(
            blocks,
            *current,
            scope,
            span,
            std::mem::take(ops),
            FunctionTerminator::Switch {
                target,
                arms,
                default,
                fallback: merge_target,
                span,
            },
        );

        for (arm_target, arm) in lowered_arms {
            let arm_scope = self.alloc_scope(Some(scope), arm.span);
            self.lower_switch_arm_body(
                arm.span,
                &arm.body,
                arm_scope,
                arm_target,
                merge_target,
                blocks,
            );
        }

        *current = merge_target;
    }

    fn lower_switch_arm_body(
        &mut self,
        span: Span,
        body: &TypedSwitchArmBody,
        scope: FunctionScopeId,
        entry: FunctionBlockId,
        merge_target: FunctionBlockId,
        blocks: &mut Vec<FunctionBlock>,
    ) {
        match body {
            TypedSwitchArmBody::Expr(expr) => {
                let mut current = entry;
                let mut ops = Vec::new();
                self.lower_expr_stmt(span, expr, scope, &mut current, &mut ops, blocks);
                self.finish_block(
                    blocks,
                    current,
                    scope,
                    span,
                    ops,
                    FunctionTerminator::Branch {
                        target: merge_target,
                        span,
                    },
                );
            }
            TypedSwitchArmBody::Stmt(stmt) => {
                let mut current = entry;
                let mut ops = Vec::new();
                if !self.lower_stmt_into(stmt, scope, &mut current, &mut ops, blocks) {
                    self.finish_block(
                        blocks,
                        current,
                        scope,
                        span,
                        ops,
                        FunctionTerminator::Branch {
                            target: merge_target,
                            span,
                        },
                    );
                }
            }
            TypedSwitchArmBody::Block(body) => {
                self.lower_body_into(
                    body,
                    entry,
                    scope,
                    blocks,
                    Fallthrough::Branch(merge_target),
                );
            }
        }
    }

    fn lower_for_stmt(
        &mut self,
        span: Span,
        for_stmt: &nia_body_ir::TypedFor,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) {
        self.push_for_init_ops(&for_stmt.header, scope, current, ops, blocks);
        let loop_header = self.alloc_block();
        self.finish_block(
            blocks,
            *current,
            scope,
            span,
            std::mem::take(ops),
            FunctionTerminator::Next {
                target: loop_header,
                span,
            },
        );

        let mut header_ops = Vec::new();
        let mut header_current = loop_header;
        let header = self.lower_loop_header(
            &for_stmt.header,
            scope,
            &mut header_current,
            &mut header_ops,
            blocks,
        );
        let body_entry = self.alloc_block();
        let continue_target = self.alloc_block();
        let break_target = self.alloc_block();
        self.finish_block(
            blocks,
            header_current,
            scope,
            span,
            header_ops,
            FunctionTerminator::Loop {
                header,
                body: body_entry,
                continue_target,
                break_target,
                span,
            },
        );

        self.loop_targets.push(LoopTargetIds {
            break_target,
            continue_target,
        });
        let body_scope = self.alloc_scope(Some(scope), for_stmt.body.span);
        self.lower_body_into(
            &for_stmt.body,
            body_entry,
            body_scope,
            blocks,
            Fallthrough::Branch(continue_target),
        );
        self.loop_targets.pop();

        self.lower_for_step(
            &for_stmt.header,
            scope,
            continue_target,
            loop_header,
            span,
            blocks,
        );
        *current = break_target;
    }

    fn finish_block(
        &mut self,
        blocks: &mut Vec<FunctionBlock>,
        current: FunctionBlockId,
        scope: FunctionScopeId,
        span: Span,
        ops: Vec<FunctionOp>,
        terminator: FunctionTerminator,
    ) {
        if ops.is_empty() {
            blocks.push(FunctionBlock {
                id: current,
                scope,
                span,
                ops,
                terminator,
            });
        } else {
            let term_block = self.alloc_block();
            blocks.push(FunctionBlock {
                id: current,
                scope,
                span,
                ops,
                terminator: FunctionTerminator::Next {
                    target: term_block,
                    span,
                },
            });
            blocks.push(FunctionBlock {
                id: term_block,
                scope,
                span,
                ops: Vec::new(),
                terminator,
            });
        }
    }

    fn finish_fallthrough_block(
        &mut self,
        blocks: &mut Vec<FunctionBlock>,
        mut current: FunctionBlockId,
        scope: FunctionScopeId,
        body: &TypedBody,
        mut ops: Vec<FunctionOp>,
        fallthrough: Fallthrough,
    ) {
        let span = body
            .tail
            .as_ref()
            .map(|tail| tail.span)
            .unwrap_or(body.span);
        let terminator = match fallthrough {
            Fallthrough::Tail => FunctionTerminator::Tail {
                value: body
                    .tail
                    .as_ref()
                    .map(|tail| self.lower_value_expr(tail, scope, &mut current, &mut ops, blocks)),
                span,
            },
            Fallthrough::Branch(target) => {
                if let Some(tail) = &body.tail {
                    let tail = self.lower_value_expr(tail, scope, &mut current, &mut ops, blocks);
                    ops.push(FunctionOp::Expr(tail));
                }
                FunctionTerminator::Branch { target, span }
            }
            Fallthrough::StoreThenBranch { local_id, target } => {
                if let Some(tail) = &body.tail {
                    let value = self.lower_value_expr(tail, scope, &mut current, &mut ops, blocks);
                    ops.push(FunctionOp::StoreLocal {
                        local_id,
                        value,
                        span: tail.span,
                    });
                }
                FunctionTerminator::Branch { target, span }
            }
        };
        self.finish_block(blocks, current, scope, span, ops, terminator);
    }

    fn lower_value_expr(
        &mut self,
        expr: &TypedExpr,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionExpr {
        let kind = match &expr.kind {
            TypedExprKind::Block(body) => {
                return self.lower_value_block_expr(expr, body, scope, current, ops, blocks);
            }
            TypedExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                return self.lower_value_if_expr(
                    expr,
                    cond,
                    then_branch,
                    else_branch.as_deref(),
                    scope,
                    current,
                    ops,
                    blocks,
                );
            }
            TypedExprKind::Switch(switch) => {
                return self.lower_value_switch_expr(expr, switch, scope, current, ops, blocks);
            }
            TypedExprKind::Len(inner) => FunctionExprKind::Len(Box::new(
                self.lower_value_expr(inner, scope, current, ops, blocks),
            )),
            TypedExprKind::Ptr(inner) => FunctionExprKind::Ptr(Box::new(
                self.lower_value_expr(inner, scope, current, ops, blocks),
            )),
            TypedExprKind::CStringPointer { array, is_const } => FunctionExprKind::CStringPointer {
                array: Box::new(self.lower_value_expr(array, scope, current, ops, blocks)),
                is_const: *is_const,
            },
            TypedExprKind::ArrayLiteral { elems } => FunctionExprKind::ArrayLiteral {
                elems: self.lower_array_elements(elems, scope, current, ops, blocks),
            },
            TypedExprKind::StructLiteral { def_id, fields } => FunctionExprKind::StructLiteral {
                def_id: *def_id,
                fields: fields
                    .iter()
                    .map(|field| FunctionFieldInit {
                        field: field.field,
                        name: field.name.clone(),
                        value: self.lower_value_expr(&field.value, scope, current, ops, blocks),
                        span: field.span,
                    })
                    .collect(),
            },
            TypedExprKind::UnionLiteral { def_id, field } => FunctionExprKind::UnionLiteral {
                def_id: *def_id,
                field: Box::new(FunctionFieldInit {
                    field: field.field,
                    name: field.name.clone(),
                    value: self.lower_value_expr(&field.value, scope, current, ops, blocks),
                    span: field.span,
                }),
            },
            TypedExprKind::Unary { op, expr: inner } => FunctionExprKind::Unary {
                op: *op,
                expr: Box::new(self.lower_value_expr(inner, scope, current, ops, blocks)),
            },
            TypedExprKind::Binary { lhs, op, rhs } => FunctionExprKind::Binary {
                lhs: Box::new(self.lower_value_expr(lhs, scope, current, ops, blocks)),
                op: *op,
                rhs: Box::new(self.lower_value_expr(rhs, scope, current, ops, blocks)),
            },
            TypedExprKind::Assign { place, op, rhs } => FunctionExprKind::Assign {
                place: self.lower_place(place, scope, current, ops, blocks),
                op: *op,
                rhs: Box::new(self.lower_value_expr(rhs, scope, current, ops, blocks)),
            },
            TypedExprKind::Discard(inner) => FunctionExprKind::Discard(Box::new(
                self.lower_value_expr(inner, scope, current, ops, blocks),
            )),
            TypedExprKind::Cast { expr: inner, ty } => FunctionExprKind::Cast {
                expr: Box::new(self.lower_value_expr(inner, scope, current, ops, blocks)),
                ty: *ty,
            },
            TypedExprKind::Call { callee, args } => FunctionExprKind::Call {
                callee: self.lower_callee(callee, scope, current, ops, blocks),
                args: args
                    .iter()
                    .map(|arg| self.lower_value_expr(arg, scope, current, ops, blocks))
                    .collect(),
            },
            TypedExprKind::Field { lhs, field } => FunctionExprKind::Field {
                lhs: Box::new(self.lower_value_expr(lhs, scope, current, ops, blocks)),
                field: *field,
            },
            TypedExprKind::Index { lhs, index } => FunctionExprKind::Index {
                lhs: Box::new(self.lower_value_expr(lhs, scope, current, ops, blocks)),
                index: Box::new(self.lower_value_expr(index, scope, current, ops, blocks)),
            },
            TypedExprKind::Slice {
                lhs,
                range,
                is_const,
            } => FunctionExprKind::Slice {
                lhs: Box::new(self.lower_value_expr(lhs, scope, current, ops, blocks)),
                range: self.lower_slice_range(range, scope, current, ops, blocks),
                is_const: *is_const,
            },
            TypedExprKind::InlineAsm(asm) => {
                FunctionExprKind::InlineAsm(self.lower_inline_asm(asm, scope, current, ops, blocks))
            }
            TypedExprKind::Error => FunctionExprKind::Error,
            TypedExprKind::Integer(text) => FunctionExprKind::Integer(text.clone()),
            TypedExprKind::Float(text) => FunctionExprKind::Float(text.clone()),
            TypedExprKind::String(scalars) => FunctionExprKind::String(scalars.clone()),
            TypedExprKind::ByteString(bytes) => FunctionExprKind::ByteString(bytes.clone()),
            TypedExprKind::Char(value) => FunctionExprKind::Char(*value),
            TypedExprKind::ByteChar(text) => FunctionExprKind::ByteChar(text.clone()),
            TypedExprKind::Bool(value) => FunctionExprKind::Bool(*value),
            TypedExprKind::Local(local_id) => FunctionExprKind::Local(*local_id),
            TypedExprKind::Global(def_id) => FunctionExprKind::Global(*def_id),
            TypedExprKind::Function(def_id) => FunctionExprKind::Function(*def_id),
            TypedExprKind::FunctionInstance { def_id, args } => {
                FunctionExprKind::FunctionInstance {
                    def_id: *def_id,
                    args: args.clone(),
                }
            }
            TypedExprKind::EnumVariant(def_id) => FunctionExprKind::EnumVariant(*def_id),
            TypedExprKind::BuiltinValue(value) => FunctionExprKind::BuiltinValue(value.clone()),
        };
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
            kind,
        }
    }

    fn lower_defer_expr(&mut self, expr: &TypedExpr) -> FunctionDeferBody {
        let scope_start = self.scopes.len();
        let root_scope = self.alloc_scope(None, expr.span);
        let entry = self.alloc_block();
        let mut current = entry;
        let mut ops = Vec::new();
        let mut blocks = Vec::new();
        self.lower_effect_expr(expr, root_scope, &mut current, &mut ops, &mut blocks);
        self.finish_block(
            &mut blocks,
            current,
            root_scope,
            expr.span,
            ops,
            FunctionTerminator::Tail {
                value: None,
                span: expr.span,
            },
        );
        let scopes = self.scopes[scope_start..].to_vec();
        self.scopes.truncate(scope_start);
        FunctionDeferBody {
            span: expr.span,
            scopes,
            blocks,
            entry,
        }
    }

    fn lower_effect_expr(
        &mut self,
        expr: &TypedExpr,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) {
        match &expr.kind {
            TypedExprKind::Block(body) => {
                let body_entry = self.alloc_block();
                let after_block = self.alloc_block();
                self.finish_block(
                    blocks,
                    *current,
                    scope,
                    expr.span,
                    std::mem::take(ops),
                    FunctionTerminator::Next {
                        target: body_entry,
                        span: expr.span,
                    },
                );
                let body_scope = self.alloc_scope(Some(scope), body.span);
                self.lower_body_into(
                    body,
                    body_entry,
                    body_scope,
                    blocks,
                    Fallthrough::Branch(after_block),
                );
                *current = after_block;
            }
            TypedExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.lower_if_expr_stmt(
                expr.span,
                cond,
                then_branch,
                else_branch.as_deref(),
                scope,
                current,
                ops,
                blocks,
            ),
            TypedExprKind::Switch(switch) => {
                self.lower_switch_expr_stmt(expr.span, switch, scope, current, ops, blocks);
            }
            _ => {
                let expr = self.lower_value_expr(expr, scope, current, ops, blocks);
                ops.push(FunctionOp::Expr(expr));
            }
        }
    }

    fn lower_value_block_expr(
        &mut self,
        expr: &TypedExpr,
        body: &TypedBody,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionExpr {
        let local = self.alloc_temp_local(expr.span, expr.ty);
        let body_entry = self.alloc_block();
        let merge_target = self.alloc_block();
        self.finish_block(
            blocks,
            *current,
            scope,
            expr.span,
            std::mem::take(ops),
            FunctionTerminator::Next {
                target: body_entry,
                span: expr.span,
            },
        );
        let body_scope = self.alloc_scope(Some(scope), body.span);
        self.lower_body_into(
            body,
            body_entry,
            body_scope,
            blocks,
            Fallthrough::StoreThenBranch {
                local_id: local,
                target: merge_target,
            },
        );
        *current = merge_target;
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
            kind: FunctionExprKind::Local(local),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_value_if_expr(
        &mut self,
        expr: &TypedExpr,
        cond: &TypedExpr,
        then_branch: &TypedBody,
        else_branch: Option<&TypedExpr>,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionExpr {
        let cond = self.lower_value_expr(cond, scope, current, ops, blocks);
        let local = self.alloc_temp_local(expr.span, expr.ty);
        let then_target = self.alloc_block();
        let else_target = self.alloc_block();
        let merge_target = self.alloc_block();
        self.finish_block(
            blocks,
            *current,
            scope,
            expr.span,
            std::mem::take(ops),
            FunctionTerminator::If {
                cond,
                then_target,
                else_target,
                span: expr.span,
            },
        );

        let then_scope = self.alloc_scope(Some(scope), then_branch.span);
        self.lower_body_into(
            then_branch,
            then_target,
            then_scope,
            blocks,
            Fallthrough::StoreThenBranch {
                local_id: local,
                target: merge_target,
            },
        );

        let mut else_current = else_target;
        let mut else_ops = Vec::new();
        if let Some(else_branch) = else_branch {
            let value =
                self.lower_value_expr(else_branch, scope, &mut else_current, &mut else_ops, blocks);
            else_ops.push(FunctionOp::StoreLocal {
                local_id: local,
                value,
                span: else_branch.span,
            });
        }
        self.finish_block(
            blocks,
            else_current,
            scope,
            else_branch.map(|expr| expr.span).unwrap_or(expr.span),
            else_ops,
            FunctionTerminator::Branch {
                target: merge_target,
                span: else_branch.map(|expr| expr.span).unwrap_or(expr.span),
            },
        );

        *current = merge_target;
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
            kind: FunctionExprKind::Local(local),
        }
    }

    fn lower_value_switch_expr(
        &mut self,
        expr: &TypedExpr,
        switch: &TypedSwitch,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionExpr {
        let target = self.lower_value_expr(&switch.target, scope, current, ops, blocks);
        let local = self.alloc_temp_local(expr.span, expr.ty);
        let merge_target = self.alloc_block();
        let mut arms = Vec::new();
        let mut lowered_arms = Vec::new();
        let mut default = None;
        for arm in &switch.arms {
            let arm_target = self.alloc_block();
            match &arm.pattern {
                TypedSwitchPattern::Expr(pattern) => arms.push(FunctionSwitchArm {
                    pattern: self.lower_value_expr(pattern, scope, current, ops, blocks),
                    target: arm_target,
                }),
                TypedSwitchPattern::Default => default = Some(arm_target),
            }
            lowered_arms.push((arm_target, arm));
        }

        self.finish_block(
            blocks,
            *current,
            scope,
            expr.span,
            std::mem::take(ops),
            FunctionTerminator::Switch {
                target,
                arms,
                default,
                fallback: merge_target,
                span: expr.span,
            },
        );

        for (arm_target, arm) in lowered_arms {
            let arm_scope = self.alloc_scope(Some(scope), arm.span);
            self.lower_value_switch_arm_body(
                arm.span,
                &arm.body,
                arm_scope,
                arm_target,
                local,
                merge_target,
                blocks,
            );
        }

        *current = merge_target;
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
            kind: FunctionExprKind::Local(local),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_value_switch_arm_body(
        &mut self,
        span: Span,
        body: &TypedSwitchArmBody,
        scope: FunctionScopeId,
        entry: FunctionBlockId,
        local_id: LocalId,
        merge_target: FunctionBlockId,
        blocks: &mut Vec<FunctionBlock>,
    ) {
        match body {
            TypedSwitchArmBody::Expr(expr) => {
                let mut current = entry;
                let mut ops = Vec::new();
                let value = self.lower_value_expr(expr, scope, &mut current, &mut ops, blocks);
                ops.push(FunctionOp::StoreLocal {
                    local_id,
                    value,
                    span: expr.span,
                });
                self.finish_block(
                    blocks,
                    current,
                    scope,
                    span,
                    ops,
                    FunctionTerminator::Branch {
                        target: merge_target,
                        span,
                    },
                );
            }
            TypedSwitchArmBody::Stmt(stmt) => {
                let mut current = entry;
                let mut ops = Vec::new();
                if !self.lower_stmt_into(stmt, scope, &mut current, &mut ops, blocks) {
                    self.finish_block(
                        blocks,
                        current,
                        scope,
                        span,
                        ops,
                        FunctionTerminator::Branch {
                            target: merge_target,
                            span,
                        },
                    );
                }
            }
            TypedSwitchArmBody::Block(body) => {
                self.lower_body_into(
                    body,
                    entry,
                    scope,
                    blocks,
                    Fallthrough::StoreThenBranch {
                        local_id,
                        target: merge_target,
                    },
                );
            }
        }
    }

    fn lower_array_elements(
        &mut self,
        elems: &TypedArrayElements,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionArrayElements {
        match elems {
            TypedArrayElements::List(elems) => FunctionArrayElements::List(
                elems
                    .iter()
                    .map(|elem| self.lower_value_expr(elem, scope, current, ops, blocks))
                    .collect(),
            ),
            TypedArrayElements::Repeat { value, count } => FunctionArrayElements::Repeat {
                value: Box::new(self.lower_value_expr(value, scope, current, ops, blocks)),
                count: *count,
            },
        }
    }

    fn lower_callee(
        &mut self,
        callee: &TypedCallee,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionCallee {
        match callee {
            TypedCallee::Function(def_id) => FunctionCallee::Function(*def_id),
            TypedCallee::FunctionInstance { def_id, args } => FunctionCallee::FunctionInstance {
                def_id: *def_id,
                args: args.clone(),
            },
            TypedCallee::Method {
                def_id,
                args,
                receiver,
            } => FunctionCallee::Method {
                def_id: *def_id,
                args: args.clone(),
                receiver: Box::new(self.lower_value_expr(receiver, scope, current, ops, blocks)),
            },
            TypedCallee::FunctionPointer(expr) => FunctionCallee::FunctionPointer(Box::new(
                self.lower_value_expr(expr, scope, current, ops, blocks),
            )),
        }
    }

    fn lower_slice_range(
        &mut self,
        range: &TypedSliceRange,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionSliceRange {
        FunctionSliceRange {
            start: range
                .start
                .as_ref()
                .map(|expr| Box::new(self.lower_value_expr(expr, scope, current, ops, blocks))),
            end: range
                .end
                .as_ref()
                .map(|expr| Box::new(self.lower_value_expr(expr, scope, current, ops, blocks))),
            inclusive: range.inclusive,
        }
    }

    fn lower_inline_asm(
        &mut self,
        asm: &TypedInlineAsm,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionInlineAsm {
        FunctionInlineAsm {
            code: asm.code.clone(),
            inputs: asm
                .inputs
                .iter()
                .map(|input| FunctionAsmInput {
                    constraint: input.constraint.clone(),
                    value: self.lower_value_expr(&input.value, scope, current, ops, blocks),
                    span: input.span,
                })
                .collect(),
            outputs: asm
                .outputs
                .iter()
                .map(|output| FunctionAsmOutput {
                    constraint: output.constraint.clone(),
                    place: self.lower_place(&output.place, scope, current, ops, blocks),
                    span: output.span,
                })
                .collect(),
            clobbers: asm.clobbers.clone(),
            options: asm.options.clone(),
        }
    }

    fn lower_place(
        &mut self,
        place: &TypedPlace,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionPlace {
        FunctionPlace {
            span: place.span,
            ty: place.ty,
            base: match &place.base {
                PlaceBase::Local(local_id) => FunctionPlaceBase::Local(*local_id),
                PlaceBase::Global(def_id) => FunctionPlaceBase::Global(*def_id),
                PlaceBase::Deref(expr) => FunctionPlaceBase::Deref(Box::new(
                    self.lower_value_expr(expr, scope, current, ops, blocks),
                )),
            },
            elems: place
                .elems
                .iter()
                .map(|elem| match elem {
                    PlaceElem::Field(field) => FunctionPlaceElem::Field(*field),
                    PlaceElem::Index(index) => FunctionPlaceElem::Index(Box::new(
                        self.lower_value_expr(index, scope, current, ops, blocks),
                    )),
                })
                .collect(),
        }
    }

    fn lower_binding(
        &mut self,
        binding: &TypedBinding,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionBinding {
        FunctionBinding {
            local_id: binding.local_id,
            name: binding.name.clone(),
            ty: binding.ty,
            value: binding
                .value
                .as_ref()
                .map(|value| self.lower_value_expr(value, scope, current, ops, blocks)),
            is_const: binding.is_const,
        }
    }

    fn push_for_init_ops(
        &mut self,
        header: &TypedForHeader,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) {
        if let TypedForHeader::CStyle {
            init: Some(init), ..
        } = header
        {
            match &**init {
                TypedForInit::Binding(binding) => {
                    let binding = self.lower_binding(binding, scope, current, ops, blocks);
                    ops.push(FunctionOp::Binding(binding));
                }
                TypedForInit::Expr(expr) => {
                    let expr = self.lower_value_expr(expr, scope, current, ops, blocks);
                    ops.push(FunctionOp::Expr(expr));
                }
            }
        }
    }

    fn lower_for_step(
        &mut self,
        header: &TypedForHeader,
        scope: FunctionScopeId,
        entry: FunctionBlockId,
        loop_header: FunctionBlockId,
        span: Span,
        blocks: &mut Vec<FunctionBlock>,
    ) {
        let mut current = entry;
        let mut ops = Vec::new();
        if let TypedForHeader::CStyle {
            step: Some(step), ..
        } = header
        {
            let step = self.lower_value_expr(step, scope, &mut current, &mut ops, blocks);
            ops.push(FunctionOp::Expr(step));
        }
        self.finish_block(
            blocks,
            current,
            scope,
            span,
            ops,
            FunctionTerminator::Branch {
                target: loop_header,
                span,
            },
        );
    }

    fn lower_loop_header(
        &mut self,
        header: &TypedForHeader,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionForHeader {
        match header {
            TypedForHeader::Infinite => FunctionForHeader::Infinite,
            TypedForHeader::Condition(cond) => FunctionForHeader::Condition(
                self.lower_value_expr(cond, scope, current, ops, blocks),
            ),
            TypedForHeader::CStyle { cond, .. } => FunctionForHeader::CStyle {
                cond: cond
                    .as_ref()
                    .map(|cond| Box::new(self.lower_value_expr(cond, scope, current, ops, blocks))),
            },
        }
    }

    fn alloc_temp_local(&mut self, span: Span, ty: InternedTyId) -> LocalId {
        let id = LocalId(self.next_temp_local);
        self.next_temp_local += 1;
        self.temp_locals.push(TypedLocal {
            id,
            name: format!("fir.tmp.{}", id.0),
            kind: TypedLocalKind::Binding,
            ty,
            span,
        });
        id
    }

    fn alloc_block(&mut self) -> FunctionBlockId {
        let id = FunctionBlockId(self.next_block);
        self.next_block += 1;
        id
    }

    fn alloc_scope(&mut self, parent: Option<FunctionScopeId>, span: Span) -> FunctionScopeId {
        let id = FunctionScopeId(self.next_scope);
        self.next_scope += 1;
        self.scopes.push(FunctionScope { id, parent, span });
        id
    }

    fn collect_body_locals(&self, body: &TypedBody, locals: &mut Vec<TypedLocal>) {
        self.extend_unique_locals(&body.locals, locals);
        self.collect_nested_body_locals(body, locals);
    }

    fn collect_nested_body_locals(&self, body: &TypedBody, locals: &mut Vec<TypedLocal>) {
        for stmt in &body.stmts {
            match &stmt.kind {
                TypedStmtKind::For(for_stmt) => self.collect_body_locals(&for_stmt.body, locals),
                TypedStmtKind::Expr(expr) => self.collect_expr_locals(expr, locals),
                TypedStmtKind::Return(Some(expr)) | TypedStmtKind::Defer(expr) => {
                    self.collect_expr_locals(expr, locals)
                }
                TypedStmtKind::Binding(_)
                | TypedStmtKind::Return(None)
                | TypedStmtKind::Break
                | TypedStmtKind::Continue => {}
            }
        }
        if let Some(tail) = &body.tail {
            self.collect_expr_locals(tail, locals);
        }
    }

    fn collect_expr_locals(&self, expr: &TypedExpr, locals: &mut Vec<TypedLocal>) {
        match &expr.kind {
            TypedExprKind::Block(body) => self.collect_body_locals(body, locals),
            TypedExprKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                self.collect_body_locals(then_branch, locals);
                if let Some(else_branch) = else_branch {
                    self.collect_expr_locals(else_branch, locals);
                }
            }
            TypedExprKind::Switch(switch) => {
                for arm in &switch.arms {
                    match &arm.body {
                        TypedSwitchArmBody::Expr(expr) => self.collect_expr_locals(expr, locals),
                        TypedSwitchArmBody::Stmt(stmt) => {
                            if let TypedStmtKind::Expr(expr) = &stmt.kind {
                                self.collect_expr_locals(expr, locals);
                            }
                        }
                        TypedSwitchArmBody::Block(body) => self.collect_body_locals(body, locals),
                    }
                }
            }
            _ => {}
        }
    }

    fn extend_unique_locals(&self, source: &[TypedLocal], target: &mut Vec<TypedLocal>) {
        for local in source {
            if !target.iter().any(|existing| existing.id == local.id) {
                target.push(local.clone());
            }
        }
    }

    fn next_available_local(&self, body: &TypedBody) -> u32 {
        fn visit_body(body: &TypedBody, max_id: &mut u32) {
            for local in &body.locals {
                *max_id = (*max_id).max(local.id.0.saturating_add(1));
            }
            for stmt in &body.stmts {
                match &stmt.kind {
                    TypedStmtKind::For(for_stmt) => visit_body(&for_stmt.body, max_id),
                    TypedStmtKind::Expr(expr)
                    | TypedStmtKind::Return(Some(expr))
                    | TypedStmtKind::Defer(expr) => visit_expr(expr, max_id),
                    TypedStmtKind::Binding(binding) => {
                        *max_id = (*max_id).max(binding.local_id.0.saturating_add(1));
                        if let Some(value) = &binding.value {
                            visit_expr(value, max_id);
                        }
                    }
                    TypedStmtKind::Return(None)
                    | TypedStmtKind::Break
                    | TypedStmtKind::Continue => {}
                }
            }
            if let Some(tail) = &body.tail {
                visit_expr(tail, max_id);
            }
        }

        fn visit_expr(expr: &TypedExpr, max_id: &mut u32) {
            match &expr.kind {
                TypedExprKind::Local(local_id) => {
                    *max_id = (*max_id).max(local_id.0.saturating_add(1));
                }
                TypedExprKind::Len(inner)
                | TypedExprKind::Ptr(inner)
                | TypedExprKind::CStringPointer { array: inner, .. }
                | TypedExprKind::Unary { expr: inner, .. }
                | TypedExprKind::Discard(inner)
                | TypedExprKind::Cast { expr: inner, .. } => visit_expr(inner, max_id),
                TypedExprKind::ArrayLiteral { elems } => match elems {
                    TypedArrayElements::List(elems) => {
                        for elem in elems {
                            visit_expr(elem, max_id);
                        }
                    }
                    TypedArrayElements::Repeat { value, .. } => visit_expr(value, max_id),
                },
                TypedExprKind::StructLiteral { fields, .. } => {
                    for field in fields {
                        visit_expr(&field.value, max_id);
                    }
                }
                TypedExprKind::UnionLiteral { field, .. } => visit_expr(&field.value, max_id),
                TypedExprKind::Binary { lhs, rhs, .. }
                | TypedExprKind::Index { lhs, index: rhs } => {
                    visit_expr(lhs, max_id);
                    visit_expr(rhs, max_id);
                }
                TypedExprKind::Assign { place, rhs, .. } => {
                    visit_place(place, max_id);
                    visit_expr(rhs, max_id);
                }
                TypedExprKind::Call { callee, args } => {
                    visit_callee(callee, max_id);
                    for arg in args {
                        visit_expr(arg, max_id);
                    }
                }
                TypedExprKind::Field { lhs, .. } => visit_expr(lhs, max_id),
                TypedExprKind::Slice { lhs, range, .. } => {
                    visit_expr(lhs, max_id);
                    if let Some(start) = &range.start {
                        visit_expr(start, max_id);
                    }
                    if let Some(end) = &range.end {
                        visit_expr(end, max_id);
                    }
                }
                TypedExprKind::Block(body) => visit_body(body, max_id),
                TypedExprKind::If {
                    cond,
                    then_branch,
                    else_branch,
                } => {
                    visit_expr(cond, max_id);
                    visit_body(then_branch, max_id);
                    if let Some(else_branch) = else_branch {
                        visit_expr(else_branch, max_id);
                    }
                }
                TypedExprKind::Switch(switch) => {
                    visit_expr(&switch.target, max_id);
                    for arm in &switch.arms {
                        if let TypedSwitchPattern::Expr(pattern) = &arm.pattern {
                            visit_expr(pattern, max_id);
                        }
                        match &arm.body {
                            TypedSwitchArmBody::Expr(expr) => visit_expr(expr, max_id),
                            TypedSwitchArmBody::Stmt(stmt) => match &stmt.kind {
                                TypedStmtKind::Expr(expr)
                                | TypedStmtKind::Return(Some(expr))
                                | TypedStmtKind::Defer(expr) => visit_expr(expr, max_id),
                                TypedStmtKind::Binding(binding) => {
                                    *max_id = (*max_id).max(binding.local_id.0.saturating_add(1));
                                    if let Some(value) = &binding.value {
                                        visit_expr(value, max_id);
                                    }
                                }
                                TypedStmtKind::For(for_stmt) => visit_body(&for_stmt.body, max_id),
                                TypedStmtKind::Return(None)
                                | TypedStmtKind::Break
                                | TypedStmtKind::Continue => {}
                            },
                            TypedSwitchArmBody::Block(body) => visit_body(body, max_id),
                        }
                    }
                }
                TypedExprKind::InlineAsm(asm) => {
                    for input in &asm.inputs {
                        visit_expr(&input.value, max_id);
                    }
                    for output in &asm.outputs {
                        visit_place(&output.place, max_id);
                    }
                }
                TypedExprKind::Error
                | TypedExprKind::Integer(_)
                | TypedExprKind::Float(_)
                | TypedExprKind::String(_)
                | TypedExprKind::ByteString(_)
                | TypedExprKind::Char(_)
                | TypedExprKind::ByteChar(_)
                | TypedExprKind::Bool(_)
                | TypedExprKind::Global(_)
                | TypedExprKind::Function(_)
                | TypedExprKind::FunctionInstance { .. }
                | TypedExprKind::EnumVariant(_)
                | TypedExprKind::BuiltinValue(_) => {}
            }
        }

        fn visit_callee(callee: &TypedCallee, max_id: &mut u32) {
            match callee {
                TypedCallee::Method { receiver, .. } | TypedCallee::FunctionPointer(receiver) => {
                    visit_expr(receiver, max_id);
                }
                TypedCallee::Function(_) | TypedCallee::FunctionInstance { .. } => {}
            }
        }

        fn visit_place(place: &TypedPlace, max_id: &mut u32) {
            if let PlaceBase::Local(local_id) = place.base {
                *max_id = (*max_id).max(local_id.0.saturating_add(1));
            }
            if let PlaceBase::Deref(expr) = &place.base {
                visit_expr(expr, max_id);
            }
            for elem in &place.elems {
                if let PlaceElem::Index(index) = elem {
                    visit_expr(index, max_id);
                }
            }
        }

        let mut max_id = 0;
        visit_body(body, &mut max_id);
        max_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_body_ir::{TypedExprKind, TypedLocalKind, TypedStmt};
    use nia_ids::{LocalId, ModuleId, TyInternerIndex};

    fn only_next_target(function_body: &FunctionBody, block: FunctionBlockId) -> FunctionBlockId {
        let FunctionTerminator::Next { target, .. } = function_body
            .block(block)
            .expect("function block")
            .terminator
        else {
            panic!("expected next terminator");
        };
        target
    }

    #[test]
    fn lowers_body_to_entry_block_with_tail() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let body = TypedBody {
            span,
            locals: vec![TypedLocal {
                id: LocalId(0),
                name: "x".to_string(),
                kind: TypedLocalKind::Binding,
                ty,
                span,
            }],
            stmts: Vec::new(),
            tail: Some(Box::new(TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Integer("1".to_string()),
            })),
            ty,
        };

        let function_body = lower_function_body(&body);

        assert_eq!(function_body.entry, FunctionBlockId(0));
        assert_eq!(function_body.blocks.len(), 1);
        assert!(function_body.blocks[0].ops.is_empty());
        assert!(matches!(
            function_body.blocks[0].terminator,
            FunctionTerminator::Tail { value: Some(_), .. }
        ));
    }

    #[test]
    fn non_terminal_ops_branch_to_tail_block() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let expr = TypedExpr {
            span,
            ty,
            kind: TypedExprKind::Integer("1".to_string()),
        };
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::Expr(expr.clone()),
            }],
            tail: Some(Box::new(expr)),
            ty,
        };

        let function_body = lower_function_body(&body);

        assert_eq!(function_body.blocks.len(), 2);
        assert_eq!(function_body.blocks[0].ops.len(), 1);
        assert!(matches!(
            function_body.blocks[0].terminator,
            FunctionTerminator::Next {
                target: FunctionBlockId(1),
                ..
            }
        ));
        assert_eq!(
            function_body.blocks[0].terminator.successors(),
            vec![FunctionBlockId(1)]
        );
        assert!(matches!(
            function_body.blocks[1].terminator,
            FunctionTerminator::Tail { value: Some(_), .. }
        ));
    }

    #[test]
    fn return_terminates_block_before_later_statements() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let expr = TypedExpr {
            span,
            ty,
            kind: TypedExprKind::Integer("1".to_string()),
        };
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![
                TypedStmt {
                    span,
                    kind: TypedStmtKind::Return(Some(expr.clone())),
                },
                TypedStmt {
                    span,
                    kind: TypedStmtKind::Expr(expr),
                },
            ],
            tail: None,
            ty,
        };

        let function_body = lower_function_body(&body);

        assert_eq!(function_body.blocks.len(), 1);
        assert!(function_body.blocks[0].ops.is_empty());
        assert!(matches!(
            function_body.blocks[0].terminator,
            FunctionTerminator::Return { value: Some(_), .. }
        ));
    }

    #[test]
    fn resolves_break_to_loop_exit_branch() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                    header: TypedForHeader::Infinite,
                    body: TypedBody {
                        span,
                        locals: Vec::new(),
                        stmts: vec![TypedStmt {
                            span,
                            kind: TypedStmtKind::Break,
                        }],
                        tail: None,
                        ty,
                    },
                })),
            }],
            tail: None,
            ty,
        };

        let function_body = lower_function_body(&body);
        let FunctionTerminator::Next { target, .. } = function_body.blocks[0].terminator else {
            panic!("expected entry branch to loop header");
        };
        let FunctionTerminator::Loop {
            body: loop_body,
            break_target,
            ..
        } = function_body.block(target).expect("loop header").terminator
        else {
            panic!("expected loop terminator");
        };
        let loop_body = function_body
            .blocks
            .iter()
            .find(|block| block.id == loop_body)
            .expect("loop body block");

        assert_eq!(loop_body.terminator.successors(), vec![break_target]);
        assert!(matches!(
            loop_body.terminator,
            FunctionTerminator::Branch { .. }
        ));
    }

    #[test]
    fn resolves_continue_to_loop_continue_branch() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                    header: TypedForHeader::Infinite,
                    body: TypedBody {
                        span,
                        locals: Vec::new(),
                        stmts: vec![TypedStmt {
                            span,
                            kind: TypedStmtKind::Continue,
                        }],
                        tail: None,
                        ty,
                    },
                })),
            }],
            tail: None,
            ty,
        };

        let function_body = lower_function_body(&body);
        let FunctionTerminator::Next { target, .. } = function_body.blocks[0].terminator else {
            panic!("expected entry branch to loop header");
        };
        let FunctionTerminator::Loop {
            body: loop_body,
            continue_target,
            ..
        } = function_body.block(target).expect("loop header").terminator
        else {
            panic!("expected loop terminator");
        };
        let loop_body = function_body
            .blocks
            .iter()
            .find(|block| block.id == loop_body)
            .expect("loop body block");

        assert_eq!(loop_body.terminator.successors(), vec![continue_target]);
        assert!(matches!(
            loop_body.terminator,
            FunctionTerminator::Branch { .. }
        ));
    }

    #[test]
    fn lowers_c_style_for_init_step_and_edges() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let expr = TypedExpr {
            span,
            ty,
            kind: TypedExprKind::Integer("1".to_string()),
        };
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                    header: TypedForHeader::CStyle {
                        init: Some(Box::new(TypedForInit::Expr(expr.clone()))),
                        cond: Some(Box::new(expr.clone())),
                        step: Some(Box::new(expr)),
                    },
                    body: TypedBody {
                        span,
                        locals: Vec::new(),
                        stmts: Vec::new(),
                        tail: None,
                        ty,
                    },
                })),
            }],
            tail: None,
            ty,
        };

        let function_body = lower_function_body(&body);

        assert!(matches!(
            function_body.blocks[0].ops[0],
            FunctionOp::Expr(_)
        ));
        assert!(matches!(
            function_body.blocks[0].terminator,
            FunctionTerminator::Next { .. }
        ));
        let loop_target = only_next_target(&function_body, function_body.blocks[0].id);
        let loop_target = only_next_target(&function_body, loop_target);
        let loop_block = function_body.block(loop_target).expect("loop header");
        let FunctionTerminator::Loop {
            body,
            continue_target,
            break_target,
            ..
        } = loop_block.terminator
        else {
            panic!("expected loop terminator");
        };
        assert_eq!(loop_block.terminator.successors(), vec![body, break_target]);
        let continue_block = function_body
            .blocks
            .iter()
            .find(|block| block.id == continue_target)
            .expect("continue block");
        assert!(matches!(continue_block.ops[0], FunctionOp::Expr(_)));
        let step_branch = only_next_target(&function_body, continue_block.id);
        assert_eq!(
            function_body
                .block(step_branch)
                .expect("step branch block")
                .terminator
                .successors(),
            vec![loop_block.id]
        );
    }

    #[test]
    fn loop_body_gets_child_scope_with_parent_loop_edges() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                    header: TypedForHeader::Infinite,
                    body: TypedBody {
                        span,
                        locals: Vec::new(),
                        stmts: Vec::new(),
                        tail: None,
                        ty,
                    },
                })),
            }],
            tail: None,
            ty,
        };

        let function_body = lower_function_body(&body);
        let root_scope = FunctionScopeId(0);
        let loop_scope = FunctionScopeId(1);
        let loop_target = only_next_target(&function_body, function_body.blocks[0].id);
        let FunctionTerminator::Loop {
            body,
            continue_target,
            break_target,
            ..
        } = function_body
            .block(loop_target)
            .expect("loop header")
            .terminator
        else {
            panic!("expected loop terminator");
        };
        let body_block = function_body
            .blocks
            .iter()
            .find(|block| block.id == body)
            .expect("loop body block");
        let continue_block = function_body
            .blocks
            .iter()
            .find(|block| block.id == continue_target)
            .expect("continue block");
        let break_block = function_body
            .blocks
            .iter()
            .find(|block| block.id == break_target)
            .expect("break block");

        assert_eq!(function_body.scopes[0].parent, None);
        assert_eq!(function_body.scopes[1].parent, Some(root_scope));
        assert_eq!(function_body.blocks[0].scope, root_scope);
        assert_eq!(
            function_body.block(loop_target).expect("loop header").scope,
            root_scope
        );
        assert_eq!(body_block.scope, loop_scope);
        assert_eq!(continue_block.scope, root_scope);
        assert_eq!(break_block.scope, root_scope);
    }

    #[test]
    fn preserves_unique_locals_from_flattened_loop_bodies() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let outer_local = TypedLocal {
            id: LocalId(0),
            name: "outer".to_string(),
            kind: TypedLocalKind::Binding,
            ty,
            span,
        };
        let inner_local = TypedLocal {
            id: LocalId(1),
            name: "inner".to_string(),
            kind: TypedLocalKind::Binding,
            ty,
            span,
        };
        let body = TypedBody {
            span,
            locals: vec![outer_local, inner_local.clone()],
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                    header: TypedForHeader::Infinite,
                    body: TypedBody {
                        span,
                        locals: vec![inner_local],
                        stmts: Vec::new(),
                        tail: None,
                        ty,
                    },
                })),
            }],
            tail: None,
            ty,
        };

        let function_body = lower_function_body(&body);

        assert_eq!(
            function_body
                .locals
                .iter()
                .map(|local| local.id)
                .collect::<Vec<_>>(),
            vec![LocalId(0), LocalId(1)]
        );
    }

    #[test]
    fn nested_loops_resolve_break_and_continue_to_nearest_loop() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let inner_continue_loop = TypedStmt {
            span,
            kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                header: TypedForHeader::Infinite,
                body: TypedBody {
                    span,
                    locals: Vec::new(),
                    stmts: vec![TypedStmt {
                        span,
                        kind: TypedStmtKind::Continue,
                    }],
                    tail: None,
                    ty,
                },
            })),
        };
        let inner_break_loop = TypedStmt {
            span,
            kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                header: TypedForHeader::Infinite,
                body: TypedBody {
                    span,
                    locals: Vec::new(),
                    stmts: vec![TypedStmt {
                        span,
                        kind: TypedStmtKind::Break,
                    }],
                    tail: None,
                    ty,
                },
            })),
        };
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                    header: TypedForHeader::Infinite,
                    body: TypedBody {
                        span,
                        locals: Vec::new(),
                        stmts: vec![inner_continue_loop, inner_break_loop],
                        tail: None,
                        ty,
                    },
                })),
            }],
            tail: None,
            ty,
        };

        let function_body = lower_function_body(&body);
        let outer_loop = only_next_target(&function_body, function_body.blocks[0].id);
        let FunctionTerminator::Loop {
            body: outer_body, ..
        } = function_body
            .block(outer_loop)
            .expect("outer loop")
            .terminator
        else {
            panic!("expected outer loop");
        };
        let outer_body = function_body
            .blocks
            .iter()
            .find(|block| block.id == outer_body)
            .expect("outer body block");
        let first_inner_loop = only_next_target(&function_body, outer_body.id);
        let FunctionTerminator::Loop {
            body: inner_body,
            continue_target: inner_continue,
            break_target: first_inner_break,
            ..
        } = function_body
            .block(first_inner_loop)
            .expect("first inner loop")
            .terminator
        else {
            panic!("expected first inner loop");
        };
        let inner_body = function_body
            .blocks
            .iter()
            .find(|block| block.id == inner_body)
            .expect("first inner body block");

        assert_eq!(inner_body.terminator.successors(), vec![inner_continue]);

        let second_inner_loop = only_next_target(&function_body, first_inner_break);
        let second_inner_loop = function_body
            .block(second_inner_loop)
            .expect("second inner loop");
        let FunctionTerminator::Loop {
            body: inner_body,
            break_target: inner_break,
            ..
        } = second_inner_loop.terminator
        else {
            panic!("expected second inner loop");
        };
        let inner_body = function_body
            .blocks
            .iter()
            .find(|block| block.id == inner_body)
            .expect("second inner body block");

        assert_eq!(inner_body.terminator.successors(), vec![inner_break]);
    }

    #[test]
    fn nested_loop_scopes_preserve_parent_chain() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                    header: TypedForHeader::Infinite,
                    body: TypedBody {
                        span,
                        locals: Vec::new(),
                        stmts: vec![TypedStmt {
                            span,
                            kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                                header: TypedForHeader::Infinite,
                                body: TypedBody {
                                    span,
                                    locals: Vec::new(),
                                    stmts: Vec::new(),
                                    tail: None,
                                    ty,
                                },
                            })),
                        }],
                        tail: None,
                        ty,
                    },
                })),
            }],
            tail: None,
            ty,
        };

        let function_body = lower_function_body(&body);

        assert_eq!(
            function_body
                .scopes
                .iter()
                .map(|scope| (scope.id, scope.parent))
                .collect::<Vec<_>>(),
            vec![
                (FunctionScopeId(0), None),
                (FunctionScopeId(1), Some(FunctionScopeId(0))),
                (FunctionScopeId(2), Some(FunctionScopeId(1))),
            ]
        );
    }

    #[test]
    fn same_scope_edges_exit_no_scopes() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let expr = TypedExpr {
            span,
            ty,
            kind: TypedExprKind::Integer("1".to_string()),
        };
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::Expr(expr),
            }],
            tail: None,
            ty,
        };

        let function_body = lower_function_body(&body);

        assert_eq!(
            function_body.edge_exited_scopes(FunctionBlockId(0), FunctionBlockId(1)),
            Some(Vec::new())
        );
    }

    #[test]
    fn loop_body_break_edge_exits_loop_scope() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                    header: TypedForHeader::Infinite,
                    body: TypedBody {
                        span,
                        locals: Vec::new(),
                        stmts: vec![TypedStmt {
                            span,
                            kind: TypedStmtKind::Break,
                        }],
                        tail: None,
                        ty,
                    },
                })),
            }],
            tail: None,
            ty,
        };

        let function_body = lower_function_body(&body);
        let loop_target = only_next_target(&function_body, function_body.blocks[0].id);
        let FunctionTerminator::Loop {
            body, break_target, ..
        } = function_body
            .block(loop_target)
            .expect("loop header")
            .terminator
        else {
            panic!("expected loop terminator");
        };

        assert_eq!(
            function_body.edge_exited_scopes(body, break_target),
            Some(vec![FunctionScopeId(1)])
        );
    }

    #[test]
    fn sibling_scope_edge_exits_only_source_scope() {
        let body = manual_function_body_for_scope_edges();

        assert_eq!(
            body.exited_scopes_between(FunctionScopeId(1), Some(FunctionScopeId(2))),
            Some(vec![FunctionScopeId(1)])
        );
    }

    #[test]
    fn return_edge_exits_scope_chain_to_function_boundary() {
        let body = manual_function_body_for_scope_edges();

        assert_eq!(
            body.return_exited_scopes(FunctionBlockId(1)),
            Some(vec![FunctionScopeId(1), FunctionScopeId(0)])
        );
    }

    fn manual_function_body_for_scope_edges() -> FunctionBody {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        FunctionBody {
            span,
            locals: Vec::new(),
            scopes: vec![
                FunctionScope {
                    id: FunctionScopeId(0),
                    parent: None,
                    span,
                },
                FunctionScope {
                    id: FunctionScopeId(1),
                    parent: Some(FunctionScopeId(0)),
                    span,
                },
                FunctionScope {
                    id: FunctionScopeId(2),
                    parent: Some(FunctionScopeId(0)),
                    span,
                },
            ],
            blocks: vec![
                FunctionBlock {
                    id: FunctionBlockId(0),
                    scope: FunctionScopeId(0),
                    span,
                    ops: Vec::new(),
                    terminator: FunctionTerminator::Branch {
                        target: FunctionBlockId(1),
                        span,
                    },
                },
                FunctionBlock {
                    id: FunctionBlockId(1),
                    scope: FunctionScopeId(1),
                    span,
                    ops: Vec::new(),
                    terminator: FunctionTerminator::Branch {
                        target: FunctionBlockId(2),
                        span,
                    },
                },
                FunctionBlock {
                    id: FunctionBlockId(2),
                    scope: FunctionScopeId(2),
                    span,
                    ops: Vec::new(),
                    terminator: FunctionTerminator::Tail { value: None, span },
                },
            ],
            entry: FunctionBlockId(0),
            ty,
        }
    }

    #[test]
    fn lowers_statement_block_expression_into_child_scope() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let expr = TypedExpr {
            span,
            ty,
            kind: TypedExprKind::Integer("1".to_string()),
        };
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::Expr(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Block(TypedBody {
                        span,
                        locals: Vec::new(),
                        stmts: vec![TypedStmt {
                            span,
                            kind: TypedStmtKind::Defer(expr.clone()),
                        }],
                        tail: Some(Box::new(expr)),
                        ty,
                    }),
                }),
            }],
            tail: None,
            ty,
        };

        let function_body = lower_function_body(&body);

        assert_eq!(function_body.scopes[1].parent, Some(FunctionScopeId(0)));
        assert!(matches!(
            function_body.blocks[0].terminator,
            FunctionTerminator::Next {
                target: FunctionBlockId(1),
                ..
            }
        ));
        assert_eq!(function_body.blocks[1].scope, FunctionScopeId(1));
        assert!(matches!(
            function_body.blocks[1].ops[0],
            FunctionOp::Defer(_)
        ));
        assert!(matches!(
            function_body.blocks[1].ops[1],
            FunctionOp::Expr(_)
        ));
        assert_eq!(
            function_body.edge_exited_scopes(FunctionBlockId(1), FunctionBlockId(2)),
            Some(vec![FunctionScopeId(1)])
        );
        assert!(!function_body.blocks[0].ops.iter().any(|op| matches!(
            op,
            FunctionOp::Expr(FunctionExpr {
                kind: FunctionExprKind::Local(_),
                ..
            })
        )));
    }

    #[test]
    fn return_from_statement_block_exits_block_and_root_scopes() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let expr = TypedExpr {
            span,
            ty,
            kind: TypedExprKind::Integer("1".to_string()),
        };
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::Expr(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Block(TypedBody {
                        span,
                        locals: Vec::new(),
                        stmts: vec![TypedStmt {
                            span,
                            kind: TypedStmtKind::Return(Some(expr)),
                        }],
                        tail: None,
                        ty,
                    }),
                }),
            }],
            tail: None,
            ty,
        };

        let function_body = lower_function_body(&body);

        assert!(matches!(
            function_body.blocks[1].terminator,
            FunctionTerminator::Return { .. }
        ));
        assert_eq!(
            function_body.return_exited_scopes(FunctionBlockId(1)),
            Some(vec![FunctionScopeId(1), FunctionScopeId(0)])
        );
    }

    #[test]
    fn collects_unique_locals_from_statement_block_expressions() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let inner_local = TypedLocal {
            id: LocalId(1),
            name: "inner".to_string(),
            kind: TypedLocalKind::Binding,
            ty,
            span,
        };
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::Expr(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Block(TypedBody {
                        span,
                        locals: vec![inner_local],
                        stmts: Vec::new(),
                        tail: None,
                        ty,
                    }),
                }),
            }],
            tail: None,
            ty,
        };

        let function_body = lower_function_body(&body);

        assert_eq!(
            function_body
                .locals
                .iter()
                .map(|local| local.id)
                .collect::<Vec<_>>(),
            vec![LocalId(1)]
        );
    }

    #[test]
    fn collects_unique_locals_from_statement_if_arms() {
        let span = Span::default();
        let ty = test_ty();
        let then_local = TypedLocal {
            id: LocalId(1),
            name: "then_local".to_string(),
            kind: TypedLocalKind::Binding,
            ty,
            span,
        };
        let else_local = TypedLocal {
            id: LocalId(2),
            name: "else_local".to_string(),
            kind: TypedLocalKind::Binding,
            ty,
            span,
        };
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::Expr(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::If {
                        cond: Box::new(bool_expr(true)),
                        then_branch: TypedBody {
                            span,
                            locals: vec![then_local],
                            stmts: Vec::new(),
                            tail: None,
                            ty,
                        },
                        else_branch: Some(Box::new(TypedExpr {
                            span,
                            ty,
                            kind: TypedExprKind::Block(TypedBody {
                                span,
                                locals: vec![else_local],
                                stmts: Vec::new(),
                                tail: None,
                                ty,
                            }),
                        })),
                    },
                }),
            }],
            tail: None,
            ty,
        };

        let function_body = lower_function_body(&body);

        assert_eq!(
            function_body
                .locals
                .iter()
                .map(|local| local.id)
                .collect::<Vec<_>>(),
            vec![LocalId(1), LocalId(2)]
        );
    }

    #[test]
    fn lowers_statement_if_into_if_terminator_and_child_scope() {
        let span = Span::default();
        let ty = test_ty();
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::Expr(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::If {
                        cond: Box::new(bool_expr(true)),
                        then_branch: TypedBody {
                            span,
                            locals: Vec::new(),
                            stmts: vec![TypedStmt {
                                span,
                                kind: TypedStmtKind::Defer(int_expr(1)),
                            }],
                            tail: None,
                            ty,
                        },
                        else_branch: None,
                    },
                }),
            }],
            tail: None,
            ty,
        };

        let function_body = lower_function_body(&body);
        let FunctionTerminator::If {
            then_target,
            else_target,
            ..
        } = function_body.blocks[0].terminator
        else {
            panic!("expected if terminator");
        };

        assert_eq!(
            function_body.blocks[0].terminator.successors(),
            vec![then_target, else_target]
        );
        assert_eq!(then_target, FunctionBlockId(1));
        assert_eq!(else_target, FunctionBlockId(2));
        assert_eq!(
            function_body
                .scope(function_body.block(then_target).expect("then block").scope)
                .unwrap()
                .parent,
            Some(FunctionScopeId(0))
        );
        assert!(matches!(
            function_body.block(then_target).expect("then block").ops[0],
            FunctionOp::Defer(_)
        ));
    }

    #[test]
    fn statement_if_without_else_uses_merge_as_false_edge() {
        let span = Span::default();
        let ty = test_ty();
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![
                TypedStmt {
                    span,
                    kind: TypedStmtKind::Expr(TypedExpr {
                        span,
                        ty,
                        kind: TypedExprKind::If {
                            cond: Box::new(bool_expr(true)),
                            then_branch: empty_body(ty),
                            else_branch: None,
                        },
                    }),
                },
                TypedStmt {
                    span,
                    kind: TypedStmtKind::Expr(int_expr(1)),
                },
            ],
            tail: None,
            ty,
        };

        let function_body = lower_function_body(&body);
        let FunctionTerminator::If { else_target, .. } = function_body.blocks[0].terminator else {
            panic!("expected if terminator");
        };
        let merge = function_body.block(else_target).expect("merge block");

        assert_eq!(merge.scope, FunctionScopeId(0));
        assert!(matches!(merge.ops[0], FunctionOp::Expr(_)));
    }

    #[test]
    fn statement_if_with_else_block_exits_else_scope_to_merge() {
        let span = Span::default();
        let ty = test_ty();
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::Expr(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::If {
                        cond: Box::new(bool_expr(true)),
                        then_branch: empty_body(ty),
                        else_branch: Some(Box::new(TypedExpr {
                            span,
                            ty,
                            kind: TypedExprKind::Block(TypedBody {
                                span,
                                locals: Vec::new(),
                                stmts: vec![TypedStmt {
                                    span,
                                    kind: TypedStmtKind::Defer(int_expr(2)),
                                }],
                                tail: None,
                                ty,
                            }),
                        })),
                    },
                }),
            }],
            tail: None,
            ty,
        };

        let function_body = lower_function_body(&body);
        let FunctionTerminator::If { else_target, .. } = function_body.blocks[0].terminator else {
            panic!("expected if terminator");
        };
        let else_entry = function_body.block(else_target).expect("else entry block");
        let FunctionTerminator::Next {
            target: else_body, ..
        } = else_entry.terminator
        else {
            panic!("expected else block jump");
        };
        let else_body = function_body.block(else_body).expect("else body block");
        let merge = function_body
            .blocks
            .iter()
            .find(|block| block.scope == FunctionScopeId(0) && block.id.0 > else_body.id.0)
            .expect("merge block");

        assert_eq!(else_body.scope, FunctionScopeId(2));
        assert_eq!(
            function_body.edge_exited_scopes(else_body.id, merge.id),
            Some(vec![FunctionScopeId(2)])
        );
    }

    #[test]
    fn return_from_statement_if_arm_exits_arm_and_root_scopes() {
        let span = Span::default();
        let ty = test_ty();
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::Expr(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::If {
                        cond: Box::new(bool_expr(true)),
                        then_branch: TypedBody {
                            span,
                            locals: Vec::new(),
                            stmts: vec![TypedStmt {
                                span,
                                kind: TypedStmtKind::Return(Some(int_expr(1))),
                            }],
                            tail: None,
                            ty,
                        },
                        else_branch: None,
                    },
                }),
            }],
            tail: None,
            ty,
        };

        let function_body = lower_function_body(&body);
        let FunctionTerminator::If { then_target, .. } = function_body.blocks[0].terminator else {
            panic!("expected if terminator");
        };

        assert!(matches!(
            function_body
                .block(then_target)
                .expect("then block")
                .terminator,
            FunctionTerminator::Return { .. }
        ));
        assert_eq!(
            function_body.return_exited_scopes(then_target),
            Some(vec![FunctionScopeId(1), FunctionScopeId(0)])
        );
    }

    #[test]
    fn lowers_statement_switch_into_switch_terminator() {
        let ty = test_ty();
        let body = switch_stmt_body(vec![
            switch_expr_arm(1, TypedSwitchArmBody::Expr(int_expr(10))),
            switch_default_arm(TypedSwitchArmBody::Expr(int_expr(20))),
        ]);

        let function_body = lower_function_body(&body);
        let FunctionTerminator::Switch {
            arms,
            default,
            fallback,
            ..
        } = &function_body.blocks[0].terminator
        else {
            panic!("expected switch terminator");
        };

        assert_eq!(arms.len(), 1);
        assert_eq!(arms[0].target, FunctionBlockId(2));
        assert_eq!(*default, Some(FunctionBlockId(3)));
        assert_eq!(*fallback, FunctionBlockId(1));
        assert_eq!(
            function_body.blocks[0].terminator.successors(),
            vec![FunctionBlockId(2), FunctionBlockId(3)]
        );
        assert_eq!(
            function_body
                .block(arms[0].target)
                .expect("case block")
                .scope,
            FunctionScopeId(1)
        );
        assert_eq!(
            function_body
                .block(default.unwrap())
                .expect("default block")
                .scope,
            FunctionScopeId(2)
        );
        assert_eq!(
            function_body.block(*fallback).expect("merge block").scope,
            FunctionScopeId(0)
        );
        assert_eq!(body.ty, ty);
    }

    #[test]
    fn statement_switch_without_default_falls_back_to_merge() {
        let body = switch_stmt_body(vec![switch_expr_arm(
            1,
            TypedSwitchArmBody::Expr(int_expr(10)),
        )]);

        let function_body = lower_function_body(&body);
        let FunctionTerminator::Switch {
            default, fallback, ..
        } = function_body.blocks[0].terminator
        else {
            panic!("expected switch terminator");
        };

        assert_eq!(default, None);
        assert_eq!(
            function_body.blocks[0].terminator.successors(),
            vec![FunctionBlockId(2), fallback]
        );
        assert_eq!(
            function_body.block(fallback).expect("merge block").scope,
            FunctionScopeId(0)
        );
    }

    #[test]
    fn statement_switch_arm_block_exits_arm_scope_to_merge() {
        let body = switch_stmt_body(vec![switch_expr_arm(
            1,
            TypedSwitchArmBody::Block(Box::new(TypedBody {
                span: Span::default(),
                locals: Vec::new(),
                stmts: vec![TypedStmt {
                    span: Span::default(),
                    kind: TypedStmtKind::Defer(int_expr(1)),
                }],
                tail: None,
                ty: test_ty(),
            })),
        )]);

        let function_body = lower_function_body(&body);
        let FunctionTerminator::Switch { arms, fallback, .. } = &function_body.blocks[0].terminator
        else {
            panic!("expected switch terminator");
        };
        let arm = function_body.block(arms[0].target).expect("arm block");

        assert_eq!(arm.scope, FunctionScopeId(1));
        assert!(matches!(arm.ops[0], FunctionOp::Defer(_)));
        assert_eq!(
            function_body.edge_exited_scopes(arm.id, *fallback),
            Some(vec![FunctionScopeId(1)])
        );
    }

    #[test]
    fn return_from_statement_switch_arm_exits_arm_and_root_scopes() {
        let body = switch_stmt_body(vec![switch_expr_arm(
            1,
            TypedSwitchArmBody::Stmt(Box::new(TypedStmt {
                span: Span::default(),
                kind: TypedStmtKind::Return(Some(int_expr(1))),
            })),
        )]);

        let function_body = lower_function_body(&body);
        let FunctionTerminator::Switch { arms, .. } = &function_body.blocks[0].terminator else {
            panic!("expected switch terminator");
        };

        assert!(matches!(
            function_body
                .block(arms[0].target)
                .expect("arm block")
                .terminator,
            FunctionTerminator::Return { .. }
        ));
        assert_eq!(
            function_body.return_exited_scopes(arms[0].target),
            Some(vec![FunctionScopeId(1), FunctionScopeId(0)])
        );
    }

    #[test]
    fn collects_unique_locals_from_statement_switch_arms() {
        let span = Span::default();
        let ty = test_ty();
        let arm_local = TypedLocal {
            id: LocalId(1),
            name: "arm_local".to_string(),
            kind: TypedLocalKind::Binding,
            ty,
            span,
        };
        let body = switch_stmt_body(vec![switch_expr_arm(
            1,
            TypedSwitchArmBody::Block(Box::new(TypedBody {
                span,
                locals: vec![arm_local],
                stmts: Vec::new(),
                tail: None,
                ty,
            })),
        )]);

        let function_body = lower_function_body(&body);

        assert_eq!(
            function_body
                .locals
                .iter()
                .map(|local| local.id)
                .collect::<Vec<_>>(),
            vec![LocalId(1)]
        );
    }

    fn test_ty() -> InternedTyId {
        InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0))
    }

    fn int_expr(value: i32) -> TypedExpr {
        TypedExpr {
            span: Span::default(),
            ty: test_ty(),
            kind: TypedExprKind::Integer(value.to_string()),
        }
    }

    fn bool_expr(value: bool) -> TypedExpr {
        TypedExpr {
            span: Span::default(),
            ty: test_ty(),
            kind: TypedExprKind::Bool(value),
        }
    }

    fn empty_body(ty: InternedTyId) -> TypedBody {
        TypedBody {
            span: Span::default(),
            locals: Vec::new(),
            stmts: Vec::new(),
            tail: None,
            ty,
        }
    }

    fn switch_stmt_body(arms: Vec<nia_body_ir::TypedSwitchArm>) -> TypedBody {
        let span = Span::default();
        let ty = test_ty();
        TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::Expr(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Switch(Box::new(TypedSwitch {
                        target: int_expr(1),
                        arms,
                    })),
                }),
            }],
            tail: None,
            ty,
        }
    }

    fn switch_expr_arm(value: i32, body: TypedSwitchArmBody) -> nia_body_ir::TypedSwitchArm {
        nia_body_ir::TypedSwitchArm {
            pattern: TypedSwitchPattern::Expr(int_expr(value)),
            body,
            span: Span::default(),
        }
    }

    fn switch_default_arm(body: TypedSwitchArmBody) -> nia_body_ir::TypedSwitchArm {
        nia_body_ir::TypedSwitchArm {
            pattern: TypedSwitchPattern::Default,
            body,
            span: Span::default(),
        }
    }
}
