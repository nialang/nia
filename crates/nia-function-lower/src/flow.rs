// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

impl FunctionLowerer {
    pub(super) fn lower_body_into(
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

    pub(super) fn lower_stmt_into(
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
                let Some(target) = self.loop_targets.last().map(|targets| targets.break_target)
                else {
                    self.finish_block(
                        blocks,
                        *current,
                        scope,
                        stmt.span,
                        std::mem::take(ops),
                        FunctionTerminator::Error { span: stmt.span },
                    );
                    return true;
                };
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
                let Some(target) = self
                    .loop_targets
                    .last()
                    .map(|targets| targets.continue_target)
                else {
                    self.finish_block(
                        blocks,
                        *current,
                        scope,
                        stmt.span,
                        std::mem::take(ops),
                        FunctionTerminator::Error { span: stmt.span },
                    );
                    return true;
                };
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

    pub(super) fn lower_expr_stmt(
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
                    StatementIf {
                        cond,
                        then_branch,
                        else_branch: else_branch.as_deref(),
                    },
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

    pub(super) fn lower_block_expr_stmt(
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

    pub(super) fn lower_if_expr_stmt(
        &mut self,
        span: Span,
        if_expr: StatementIf<'_>,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) {
        let cond = self.lower_value_expr(if_expr.cond, scope, current, ops, blocks);
        let then_target = self.alloc_block();
        let else_target = if_expr.else_branch.map(|_| self.alloc_block());
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

        let then_scope = self.alloc_scope(Some(scope), if_expr.then_branch.span);
        self.lower_body_into(
            if_expr.then_branch,
            then_target,
            then_scope,
            blocks,
            Fallthrough::Branch(merge_target),
        );

        if let (Some(else_branch), Some(else_target)) = (if_expr.else_branch, else_target) {
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

    pub(super) fn lower_switch_expr_stmt(
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

    pub(super) fn lower_switch_arm_body(
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

    pub(super) fn lower_for_stmt(
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

    pub(super) fn finish_block(
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

    pub(super) fn finish_fallthrough_block(
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

    pub(super) fn push_for_init_ops(
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

    pub(super) fn lower_for_step(
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

    pub(super) fn lower_loop_header(
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
}
