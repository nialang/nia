// SPDX-License-Identifier: GPL-3.0-or-later
use nia_body_ir::{
    TypedBinding, TypedBody, TypedExpr, TypedExprKind, TypedForHeader, TypedForInit, TypedLocal,
    TypedStmt, TypedStmtKind, TypedSwitch, TypedSwitchArmBody, TypedSwitchPattern,
};
use nia_ids::InternedTyId;
use nia_span::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ControlBlockId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ControlScopeId(pub u32);

#[derive(Debug, Clone, PartialEq)]
pub struct ControlBody {
    pub span: Span,
    pub locals: Vec<TypedLocal>,
    pub scopes: Vec<ControlScope>,
    pub blocks: Vec<ControlBlock>,
    pub entry: ControlBlockId,
    pub ty: InternedTyId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ControlScope {
    pub id: ControlScopeId,
    pub parent: Option<ControlScopeId>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ControlBlock {
    pub id: ControlBlockId,
    pub scope: ControlScopeId,
    pub span: Span,
    pub ops: Vec<ControlOp>,
    pub terminator: ControlTerminator,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ControlOp {
    Binding(TypedBinding),
    /// Effect-only expression. Statement-position block/if/switch expressions
    /// are lowered to control blocks; value-position control expressions remain
    /// inside `TypedExpr` until CIR grows explicit value merge nodes.
    Expr(TypedExpr),
    Defer(TypedExpr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ControlTerminator {
    Branch {
        target: ControlBlockId,
        span: Span,
    },
    Next {
        target: ControlBlockId,
        span: Span,
    },
    If {
        cond: TypedExpr,
        then_target: ControlBlockId,
        else_target: ControlBlockId,
        span: Span,
    },
    Switch {
        target: TypedExpr,
        arms: Vec<ControlSwitchArm>,
        default: Option<ControlBlockId>,
        fallback: ControlBlockId,
        span: Span,
    },
    Loop {
        header: TypedForHeader,
        body: ControlBlockId,
        continue_target: ControlBlockId,
        break_target: ControlBlockId,
        span: Span,
    },
    Return {
        value: Option<TypedExpr>,
        span: Span,
    },
    Tail {
        value: Option<TypedExpr>,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ControlSwitchArm {
    pub pattern: TypedExpr,
    pub target: ControlBlockId,
}

impl ControlTerminator {
    pub fn successors(&self) -> Vec<ControlBlockId> {
        match self {
            ControlTerminator::Branch { target, .. } | ControlTerminator::Next { target, .. } => {
                vec![*target]
            }
            ControlTerminator::If {
                then_target,
                else_target,
                ..
            } => vec![*then_target, *else_target],
            ControlTerminator::Switch {
                arms,
                default,
                fallback,
                ..
            } => arms
                .iter()
                .map(|arm| arm.target)
                .chain(default.or(Some(*fallback)))
                .collect(),
            ControlTerminator::Loop {
                body, break_target, ..
            } => vec![*body, *break_target],
            ControlTerminator::Return { .. } | ControlTerminator::Tail { .. } => Vec::new(),
        }
    }
}

impl ControlBody {
    pub fn block(&self, id: ControlBlockId) -> Option<&ControlBlock> {
        self.blocks.iter().find(|block| block.id == id)
    }

    pub fn scope(&self, id: ControlScopeId) -> Option<&ControlScope> {
        self.scopes.iter().find(|scope| scope.id == id)
    }

    pub fn edge_exited_scopes(
        &self,
        from: ControlBlockId,
        to: ControlBlockId,
    ) -> Option<Vec<ControlScopeId>> {
        let from = self.block(from)?.scope;
        let to = self.block(to)?.scope;
        self.exited_scopes_between(from, Some(to))
    }

    pub fn return_exited_scopes(&self, from: ControlBlockId) -> Option<Vec<ControlScopeId>> {
        let from = self.block(from)?.scope;
        self.exited_scopes_between(from, None)
    }

    pub fn exited_scopes_between(
        &self,
        from: ControlScopeId,
        to: Option<ControlScopeId>,
    ) -> Option<Vec<ControlScopeId>> {
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

    fn scope_chain_to_root(&self, scope: ControlScopeId) -> Option<Vec<ControlScopeId>> {
        let mut chain = Vec::new();
        let mut current = Some(scope);
        while let Some(scope) = current {
            chain.push(scope);
            current = self.scope(scope)?.parent;
        }
        Some(chain)
    }
}

pub fn lower_control_body(body: &TypedBody) -> ControlBody {
    ControlLowerer::new().lower_body(body)
}

struct ControlLowerer {
    next_block: u32,
    next_scope: u32,
    scopes: Vec<ControlScope>,
    loop_targets: Vec<LoopTargetIds>,
}

#[derive(Debug, Clone, Copy)]
struct LoopTargetIds {
    break_target: ControlBlockId,
    continue_target: ControlBlockId,
}

#[derive(Debug, Clone, Copy)]
enum Fallthrough {
    Tail,
    Branch(ControlBlockId),
}

impl ControlLowerer {
    fn new() -> Self {
        Self {
            next_block: 0,
            next_scope: 0,
            scopes: Vec::new(),
            loop_targets: Vec::new(),
        }
    }

    fn lower_body(&mut self, body: &TypedBody) -> ControlBody {
        let root_scope = self.alloc_scope(None, body.span);
        let entry = self.alloc_block();
        let mut blocks = Vec::new();
        let mut locals = Vec::new();
        self.lower_body_into(body, entry, root_scope, &mut blocks, Fallthrough::Tail);
        self.collect_body_locals(body, &mut locals);
        ControlBody {
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
        entry: ControlBlockId,
        scope: ControlScopeId,
        blocks: &mut Vec<ControlBlock>,
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
        scope: ControlScopeId,
        current: &mut ControlBlockId,
        ops: &mut Vec<ControlOp>,
        blocks: &mut Vec<ControlBlock>,
    ) -> bool {
        match &stmt.kind {
            TypedStmtKind::Binding(binding) => ops.push(ControlOp::Binding(binding.clone())),
            TypedStmtKind::Expr(expr) => {
                self.lower_expr_stmt(stmt.span, expr, scope, current, ops, blocks);
            }
            TypedStmtKind::Defer(expr) => ops.push(ControlOp::Defer(expr.clone())),
            TypedStmtKind::Return(value) => {
                self.finish_block(
                    blocks,
                    *current,
                    scope,
                    stmt.span,
                    std::mem::take(ops),
                    ControlTerminator::Return {
                        value: value.clone(),
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
                    .unwrap_or(ControlBlockId(u32::MAX));
                self.finish_block(
                    blocks,
                    *current,
                    scope,
                    stmt.span,
                    std::mem::take(ops),
                    ControlTerminator::Branch {
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
                    .unwrap_or(ControlBlockId(u32::MAX));
                self.finish_block(
                    blocks,
                    *current,
                    scope,
                    stmt.span,
                    std::mem::take(ops),
                    ControlTerminator::Branch {
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
        scope: ControlScopeId,
        current: &mut ControlBlockId,
        ops: &mut Vec<ControlOp>,
        blocks: &mut Vec<ControlBlock>,
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
            _ => ops.push(ControlOp::Expr(expr.clone())),
        }
    }

    fn lower_block_expr_stmt(
        &mut self,
        span: Span,
        body: &TypedBody,
        scope: ControlScopeId,
        current: &mut ControlBlockId,
        ops: &mut Vec<ControlOp>,
        blocks: &mut Vec<ControlBlock>,
    ) {
        let body_entry = self.alloc_block();
        let after_block = self.alloc_block();
        self.finish_block(
            blocks,
            *current,
            scope,
            span,
            std::mem::take(ops),
            ControlTerminator::Next {
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
        scope: ControlScopeId,
        current: &mut ControlBlockId,
        ops: &mut Vec<ControlOp>,
        blocks: &mut Vec<ControlBlock>,
    ) {
        let then_target = self.alloc_block();
        let else_target = else_branch.map(|_| self.alloc_block());
        let merge_target = self.alloc_block();
        self.finish_block(
            blocks,
            *current,
            scope,
            span,
            std::mem::take(ops),
            ControlTerminator::If {
                cond: cond.clone(),
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
                ControlTerminator::Branch {
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
        scope: ControlScopeId,
        current: &mut ControlBlockId,
        ops: &mut Vec<ControlOp>,
        blocks: &mut Vec<ControlBlock>,
    ) {
        let merge_target = self.alloc_block();
        let mut arms = Vec::new();
        let mut lowered_arms = Vec::new();
        let mut default = None;
        for arm in &switch.arms {
            let arm_target = self.alloc_block();
            match &arm.pattern {
                TypedSwitchPattern::Expr(pattern) => arms.push(ControlSwitchArm {
                    pattern: pattern.clone(),
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
            ControlTerminator::Switch {
                target: switch.target.clone(),
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
        scope: ControlScopeId,
        entry: ControlBlockId,
        merge_target: ControlBlockId,
        blocks: &mut Vec<ControlBlock>,
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
                    ControlTerminator::Branch {
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
                        ControlTerminator::Branch {
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
        scope: ControlScopeId,
        current: &mut ControlBlockId,
        ops: &mut Vec<ControlOp>,
        blocks: &mut Vec<ControlBlock>,
    ) {
        self.push_for_init_ops(&for_stmt.header, ops);
        let loop_header = if ops.is_empty() {
            *current
        } else {
            let loop_header = self.alloc_block();
            blocks.push(ControlBlock {
                id: *current,
                scope,
                span,
                ops: std::mem::take(ops),
                terminator: ControlTerminator::Next {
                    target: loop_header,
                    span,
                },
            });
            loop_header
        };
        let body_entry = self.alloc_block();
        let continue_target = self.alloc_block();
        let break_target = self.alloc_block();
        blocks.push(ControlBlock {
            id: loop_header,
            scope,
            span,
            ops: Vec::new(),
            terminator: ControlTerminator::Loop {
                header: self.lower_loop_header(&for_stmt.header),
                body: body_entry,
                continue_target,
                break_target,
                span,
            },
        });

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

        blocks.push(ControlBlock {
            id: continue_target,
            scope,
            span,
            ops: self.for_step_ops(&for_stmt.header),
            terminator: ControlTerminator::Branch {
                target: loop_header,
                span,
            },
        });
        *current = break_target;
    }

    fn finish_block(
        &mut self,
        blocks: &mut Vec<ControlBlock>,
        current: ControlBlockId,
        scope: ControlScopeId,
        span: Span,
        ops: Vec<ControlOp>,
        terminator: ControlTerminator,
    ) {
        if ops.is_empty() {
            blocks.push(ControlBlock {
                id: current,
                scope,
                span,
                ops,
                terminator,
            });
        } else {
            let term_block = self.alloc_block();
            blocks.push(ControlBlock {
                id: current,
                scope,
                span,
                ops,
                terminator: ControlTerminator::Next {
                    target: term_block,
                    span,
                },
            });
            blocks.push(ControlBlock {
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
        blocks: &mut Vec<ControlBlock>,
        current: ControlBlockId,
        scope: ControlScopeId,
        body: &TypedBody,
        mut ops: Vec<ControlOp>,
        fallthrough: Fallthrough,
    ) {
        let span = body
            .tail
            .as_ref()
            .map(|tail| tail.span)
            .unwrap_or(body.span);
        let terminator = match fallthrough {
            Fallthrough::Tail => ControlTerminator::Tail {
                value: body.tail.as_ref().map(|tail| (**tail).clone()),
                span,
            },
            Fallthrough::Branch(target) => {
                if let Some(tail) = &body.tail {
                    ops.push(ControlOp::Expr((**tail).clone()));
                }
                ControlTerminator::Branch { target, span }
            }
        };
        self.finish_block(blocks, current, scope, span, ops, terminator);
    }

    fn push_for_init_ops(&self, header: &TypedForHeader, ops: &mut Vec<ControlOp>) {
        if let TypedForHeader::CStyle {
            init: Some(init), ..
        } = header
        {
            match &**init {
                TypedForInit::Binding(binding) => ops.push(ControlOp::Binding(binding.clone())),
                TypedForInit::Expr(expr) => ops.push(ControlOp::Expr(expr.clone())),
            }
        }
    }

    fn for_step_ops(&self, header: &TypedForHeader) -> Vec<ControlOp> {
        match header {
            TypedForHeader::CStyle {
                step: Some(step), ..
            } => vec![ControlOp::Expr((**step).clone())],
            _ => Vec::new(),
        }
    }

    fn lower_loop_header(&self, header: &TypedForHeader) -> TypedForHeader {
        match header {
            TypedForHeader::Infinite => TypedForHeader::Infinite,
            TypedForHeader::Condition(cond) => TypedForHeader::Condition(cond.clone()),
            TypedForHeader::CStyle { cond, .. } => TypedForHeader::CStyle {
                init: None,
                cond: cond.as_ref().map(|cond| Box::new((**cond).clone())),
                step: None,
            },
        }
    }

    fn alloc_block(&mut self) -> ControlBlockId {
        let id = ControlBlockId(self.next_block);
        self.next_block += 1;
        id
    }

    fn alloc_scope(&mut self, parent: Option<ControlScopeId>, span: Span) -> ControlScopeId {
        let id = ControlScopeId(self.next_scope);
        self.next_scope += 1;
        self.scopes.push(ControlScope { id, parent, span });
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_body_ir::{TypedExprKind, TypedLocalKind, TypedStmt};
    use nia_ids::{LocalId, ModuleId, TyInternerIndex};

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

        let control = lower_control_body(&body);

        assert_eq!(control.entry, ControlBlockId(0));
        assert_eq!(control.blocks.len(), 1);
        assert!(control.blocks[0].ops.is_empty());
        assert!(matches!(
            control.blocks[0].terminator,
            ControlTerminator::Tail { value: Some(_), .. }
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

        let control = lower_control_body(&body);

        assert_eq!(control.blocks.len(), 2);
        assert_eq!(control.blocks[0].ops.len(), 1);
        assert!(matches!(
            control.blocks[0].terminator,
            ControlTerminator::Next {
                target: ControlBlockId(1),
                ..
            }
        ));
        assert_eq!(
            control.blocks[0].terminator.successors(),
            vec![ControlBlockId(1)]
        );
        assert!(matches!(
            control.blocks[1].terminator,
            ControlTerminator::Tail { value: Some(_), .. }
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

        let control = lower_control_body(&body);

        assert_eq!(control.blocks.len(), 1);
        assert!(control.blocks[0].ops.is_empty());
        assert!(matches!(
            control.blocks[0].terminator,
            ControlTerminator::Return { value: Some(_), .. }
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

        let control = lower_control_body(&body);
        let ControlTerminator::Loop {
            body: loop_body,
            break_target,
            ..
        } = control.blocks[0].terminator
        else {
            panic!("expected loop terminator");
        };
        let loop_body = control
            .blocks
            .iter()
            .find(|block| block.id == loop_body)
            .expect("loop body block");

        assert_eq!(loop_body.terminator.successors(), vec![break_target]);
        assert!(matches!(
            loop_body.terminator,
            ControlTerminator::Branch { .. }
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

        let control = lower_control_body(&body);
        let ControlTerminator::Loop {
            body: loop_body,
            continue_target,
            ..
        } = control.blocks[0].terminator
        else {
            panic!("expected loop terminator");
        };
        let loop_body = control
            .blocks
            .iter()
            .find(|block| block.id == loop_body)
            .expect("loop body block");

        assert_eq!(loop_body.terminator.successors(), vec![continue_target]);
        assert!(matches!(
            loop_body.terminator,
            ControlTerminator::Branch { .. }
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

        let control = lower_control_body(&body);

        assert!(matches!(control.blocks[0].ops[0], ControlOp::Expr(_)));
        assert!(matches!(
            control.blocks[0].terminator,
            ControlTerminator::Next { .. }
        ));
        let loop_block = &control.blocks[1];
        let ControlTerminator::Loop {
            body,
            continue_target,
            break_target,
            ..
        } = loop_block.terminator
        else {
            panic!("expected loop terminator");
        };
        assert_eq!(loop_block.terminator.successors(), vec![body, break_target]);
        let continue_block = control
            .blocks
            .iter()
            .find(|block| block.id == continue_target)
            .expect("continue block");
        assert!(matches!(continue_block.ops[0], ControlOp::Expr(_)));
        assert_eq!(continue_block.terminator.successors(), vec![loop_block.id]);
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

        let control = lower_control_body(&body);
        let root_scope = ControlScopeId(0);
        let loop_scope = ControlScopeId(1);
        let ControlTerminator::Loop {
            body,
            continue_target,
            break_target,
            ..
        } = control.blocks[0].terminator
        else {
            panic!("expected loop terminator");
        };
        let body_block = control
            .blocks
            .iter()
            .find(|block| block.id == body)
            .expect("loop body block");
        let continue_block = control
            .blocks
            .iter()
            .find(|block| block.id == continue_target)
            .expect("continue block");
        let break_block = control
            .blocks
            .iter()
            .find(|block| block.id == break_target)
            .expect("break block");

        assert_eq!(control.scopes[0].parent, None);
        assert_eq!(control.scopes[1].parent, Some(root_scope));
        assert_eq!(control.blocks[0].scope, root_scope);
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

        let control = lower_control_body(&body);

        assert_eq!(
            control
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

        let control = lower_control_body(&body);
        let ControlTerminator::Loop {
            body: outer_body, ..
        } = control.blocks[0].terminator
        else {
            panic!("expected outer loop");
        };
        let outer_body = control
            .blocks
            .iter()
            .find(|block| block.id == outer_body)
            .expect("outer body block");
        let ControlTerminator::Loop {
            body: inner_body,
            continue_target: inner_continue,
            break_target: first_inner_break,
            ..
        } = outer_body.terminator
        else {
            panic!("expected first inner loop");
        };
        let inner_body = control
            .blocks
            .iter()
            .find(|block| block.id == inner_body)
            .expect("first inner body block");

        assert_eq!(inner_body.terminator.successors(), vec![inner_continue]);

        let second_inner_loop = control
            .blocks
            .iter()
            .find(|block| block.id == first_inner_break)
            .expect("second inner loop block");
        let ControlTerminator::Loop {
            body: inner_body,
            break_target: inner_break,
            ..
        } = second_inner_loop.terminator
        else {
            panic!("expected second inner loop");
        };
        let inner_body = control
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

        let control = lower_control_body(&body);

        assert_eq!(
            control
                .scopes
                .iter()
                .map(|scope| (scope.id, scope.parent))
                .collect::<Vec<_>>(),
            vec![
                (ControlScopeId(0), None),
                (ControlScopeId(1), Some(ControlScopeId(0))),
                (ControlScopeId(2), Some(ControlScopeId(1))),
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

        let control = lower_control_body(&body);

        assert_eq!(
            control.edge_exited_scopes(ControlBlockId(0), ControlBlockId(1)),
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

        let control = lower_control_body(&body);
        let ControlTerminator::Loop {
            body, break_target, ..
        } = control.blocks[0].terminator
        else {
            panic!("expected loop terminator");
        };

        assert_eq!(
            control.edge_exited_scopes(body, break_target),
            Some(vec![ControlScopeId(1)])
        );
    }

    #[test]
    fn sibling_scope_edge_exits_only_source_scope() {
        let body = manual_control_body_for_scope_edges();

        assert_eq!(
            body.exited_scopes_between(ControlScopeId(1), Some(ControlScopeId(2))),
            Some(vec![ControlScopeId(1)])
        );
    }

    #[test]
    fn return_edge_exits_scope_chain_to_function_boundary() {
        let body = manual_control_body_for_scope_edges();

        assert_eq!(
            body.return_exited_scopes(ControlBlockId(1)),
            Some(vec![ControlScopeId(1), ControlScopeId(0)])
        );
    }

    fn manual_control_body_for_scope_edges() -> ControlBody {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        ControlBody {
            span,
            locals: Vec::new(),
            scopes: vec![
                ControlScope {
                    id: ControlScopeId(0),
                    parent: None,
                    span,
                },
                ControlScope {
                    id: ControlScopeId(1),
                    parent: Some(ControlScopeId(0)),
                    span,
                },
                ControlScope {
                    id: ControlScopeId(2),
                    parent: Some(ControlScopeId(0)),
                    span,
                },
            ],
            blocks: vec![
                ControlBlock {
                    id: ControlBlockId(0),
                    scope: ControlScopeId(0),
                    span,
                    ops: Vec::new(),
                    terminator: ControlTerminator::Branch {
                        target: ControlBlockId(1),
                        span,
                    },
                },
                ControlBlock {
                    id: ControlBlockId(1),
                    scope: ControlScopeId(1),
                    span,
                    ops: Vec::new(),
                    terminator: ControlTerminator::Branch {
                        target: ControlBlockId(2),
                        span,
                    },
                },
                ControlBlock {
                    id: ControlBlockId(2),
                    scope: ControlScopeId(2),
                    span,
                    ops: Vec::new(),
                    terminator: ControlTerminator::Tail { value: None, span },
                },
            ],
            entry: ControlBlockId(0),
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

        let control = lower_control_body(&body);

        assert_eq!(control.scopes[1].parent, Some(ControlScopeId(0)));
        assert!(matches!(
            control.blocks[0].terminator,
            ControlTerminator::Next {
                target: ControlBlockId(1),
                ..
            }
        ));
        assert_eq!(control.blocks[1].scope, ControlScopeId(1));
        assert!(matches!(control.blocks[1].ops[0], ControlOp::Defer(_)));
        assert!(matches!(control.blocks[1].ops[1], ControlOp::Expr(_)));
        assert_eq!(
            control.edge_exited_scopes(ControlBlockId(1), ControlBlockId(2)),
            Some(vec![ControlScopeId(1)])
        );
        assert!(!control.blocks[0].ops.iter().any(|op| matches!(
            op,
            ControlOp::Expr(TypedExpr {
                kind: TypedExprKind::Block(_),
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

        let control = lower_control_body(&body);

        assert!(matches!(
            control.blocks[1].terminator,
            ControlTerminator::Return { .. }
        ));
        assert_eq!(
            control.return_exited_scopes(ControlBlockId(1)),
            Some(vec![ControlScopeId(1), ControlScopeId(0)])
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

        let control = lower_control_body(&body);

        assert_eq!(
            control
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

        let control = lower_control_body(&body);

        assert_eq!(
            control
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

        let control = lower_control_body(&body);
        let ControlTerminator::If {
            then_target,
            else_target,
            ..
        } = control.blocks[0].terminator
        else {
            panic!("expected if terminator");
        };

        assert_eq!(
            control.blocks[0].terminator.successors(),
            vec![then_target, else_target]
        );
        assert_eq!(then_target, ControlBlockId(1));
        assert_eq!(else_target, ControlBlockId(2));
        assert_eq!(
            control
                .scope(control.block(then_target).expect("then block").scope)
                .unwrap()
                .parent,
            Some(ControlScopeId(0))
        );
        assert!(matches!(
            control.block(then_target).expect("then block").ops[0],
            ControlOp::Defer(_)
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

        let control = lower_control_body(&body);
        let ControlTerminator::If { else_target, .. } = control.blocks[0].terminator else {
            panic!("expected if terminator");
        };
        let merge = control.block(else_target).expect("merge block");

        assert_eq!(merge.scope, ControlScopeId(0));
        assert!(matches!(merge.ops[0], ControlOp::Expr(_)));
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

        let control = lower_control_body(&body);
        let ControlTerminator::If { else_target, .. } = control.blocks[0].terminator else {
            panic!("expected if terminator");
        };
        let else_entry = control.block(else_target).expect("else entry block");
        let ControlTerminator::Next {
            target: else_body, ..
        } = else_entry.terminator
        else {
            panic!("expected else block jump");
        };
        let else_body = control.block(else_body).expect("else body block");
        let merge = control
            .blocks
            .iter()
            .find(|block| block.scope == ControlScopeId(0) && block.id.0 > else_body.id.0)
            .expect("merge block");

        assert_eq!(else_body.scope, ControlScopeId(2));
        assert_eq!(
            control.edge_exited_scopes(else_body.id, merge.id),
            Some(vec![ControlScopeId(2)])
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

        let control = lower_control_body(&body);
        let ControlTerminator::If { then_target, .. } = control.blocks[0].terminator else {
            panic!("expected if terminator");
        };

        assert!(matches!(
            control.block(then_target).expect("then block").terminator,
            ControlTerminator::Return { .. }
        ));
        assert_eq!(
            control.return_exited_scopes(then_target),
            Some(vec![ControlScopeId(1), ControlScopeId(0)])
        );
    }

    #[test]
    fn lowers_statement_switch_into_switch_terminator() {
        let ty = test_ty();
        let body = switch_stmt_body(vec![
            switch_expr_arm(1, TypedSwitchArmBody::Expr(int_expr(10))),
            switch_default_arm(TypedSwitchArmBody::Expr(int_expr(20))),
        ]);

        let control = lower_control_body(&body);
        let ControlTerminator::Switch {
            arms,
            default,
            fallback,
            ..
        } = &control.blocks[0].terminator
        else {
            panic!("expected switch terminator");
        };

        assert_eq!(arms.len(), 1);
        assert_eq!(arms[0].target, ControlBlockId(2));
        assert_eq!(*default, Some(ControlBlockId(3)));
        assert_eq!(*fallback, ControlBlockId(1));
        assert_eq!(
            control.blocks[0].terminator.successors(),
            vec![ControlBlockId(2), ControlBlockId(3)]
        );
        assert_eq!(
            control.block(arms[0].target).expect("case block").scope,
            ControlScopeId(1)
        );
        assert_eq!(
            control
                .block(default.unwrap())
                .expect("default block")
                .scope,
            ControlScopeId(2)
        );
        assert_eq!(
            control.block(*fallback).expect("merge block").scope,
            ControlScopeId(0)
        );
        assert_eq!(body.ty, ty);
    }

    #[test]
    fn statement_switch_without_default_falls_back_to_merge() {
        let body = switch_stmt_body(vec![switch_expr_arm(
            1,
            TypedSwitchArmBody::Expr(int_expr(10)),
        )]);

        let control = lower_control_body(&body);
        let ControlTerminator::Switch {
            default, fallback, ..
        } = control.blocks[0].terminator
        else {
            panic!("expected switch terminator");
        };

        assert_eq!(default, None);
        assert_eq!(
            control.blocks[0].terminator.successors(),
            vec![ControlBlockId(2), fallback]
        );
        assert_eq!(
            control.block(fallback).expect("merge block").scope,
            ControlScopeId(0)
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

        let control = lower_control_body(&body);
        let ControlTerminator::Switch { arms, fallback, .. } = &control.blocks[0].terminator else {
            panic!("expected switch terminator");
        };
        let arm = control.block(arms[0].target).expect("arm block");

        assert_eq!(arm.scope, ControlScopeId(1));
        assert!(matches!(arm.ops[0], ControlOp::Defer(_)));
        assert_eq!(
            control.edge_exited_scopes(arm.id, *fallback),
            Some(vec![ControlScopeId(1)])
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

        let control = lower_control_body(&body);
        let ControlTerminator::Switch { arms, .. } = &control.blocks[0].terminator else {
            panic!("expected switch terminator");
        };

        assert!(matches!(
            control.block(arms[0].target).expect("arm block").terminator,
            ControlTerminator::Return { .. }
        ));
        assert_eq!(
            control.return_exited_scopes(arms[0].target),
            Some(vec![ControlScopeId(1), ControlScopeId(0)])
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

        let control = lower_control_body(&body);

        assert_eq!(
            control
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
