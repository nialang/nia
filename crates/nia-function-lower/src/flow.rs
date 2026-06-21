// SPDX-License-Identifier: GPL-3.0-or-later
use super::support::{LoweringContext, PatternConditionContext, SwitchStmtArmContext};
use super::*;
use nia_ids::TyInternerIndex;

impl FunctionLowerer {
    pub(super) fn lower_body_into(
        &mut self,
        body: &TypedBody,
        entry: FunctionBlockId,
        scope: FunctionScopeId,
        blocks: &mut Vec<FunctionBlock>,
        fallthrough: Fallthrough,
    ) {
        self.lower_body_into_with_ops(body, entry, scope, blocks, fallthrough, Vec::new());
    }

    fn lower_body_into_with_ops(
        &mut self,
        body: &TypedBody,
        entry: FunctionBlockId,
        scope: FunctionScopeId,
        blocks: &mut Vec<FunctionBlock>,
        fallthrough: Fallthrough,
        mut ops: Vec<FunctionOp>,
    ) {
        let mut current = entry;
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
            TypedStmtKind::ForIn(for_stmt) => {
                self.lower_for_in_stmt(stmt.span, for_stmt, scope, current, ops, blocks);
            }
            TypedStmtKind::While(while_stmt) => {
                self.lower_while_stmt(stmt.span, while_stmt, scope, current, ops, blocks);
            }
            TypedStmtKind::Loop(loop_stmt) => {
                self.lower_loop_stmt(stmt.span, loop_stmt, scope, current, ops, blocks);
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
            TypedExprKind::IfPattern(if_pattern) => {
                self.lower_if_pattern_expr_stmt(span, if_pattern, scope, current, ops, blocks);
            }
            TypedExprKind::Switch(switch) => {
                self.lower_switch_expr_stmt(span, switch, scope, current, ops, blocks);
            }
            TypedExprKind::MemoryIntrinsic(memory) => {
                let op = self.lower_memory_intrinsic_op(memory, scope, current, ops, blocks);
                ops.push(op);
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

    pub(super) fn lower_if_pattern_expr_stmt(
        &mut self,
        span: Span,
        if_pattern: &TypedIfPattern,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) {
        let target_value = self.lower_value_expr(&if_pattern.target, scope, current, ops, blocks);
        let target_local = self.alloc_temp_local(if_pattern.target.span, if_pattern.target.ty);
        ops.push(FunctionOp::StoreLocal {
            local_id: target_local,
            value: target_value,
            span: if_pattern.target.span,
        });
        let target = FunctionExpr {
            span: if_pattern.target.span,
            ty: if_pattern.target.ty,
            kind: FunctionExprKind::Local(target_local),
        };
        let merge_target = self.alloc_block();
        let else_target = if if_pattern.else_branch.is_some() {
            self.alloc_block()
        } else {
            merge_target
        };
        let arm_targets = if_pattern
            .arms
            .iter()
            .map(|_| self.alloc_block())
            .collect::<Vec<_>>();
        let check_blocks = if_pattern
            .arms
            .iter()
            .map(|_| self.alloc_block())
            .collect::<Vec<_>>();
        let first_target = check_blocks.first().copied().unwrap_or(else_target);
        self.finish_block(
            blocks,
            *current,
            scope,
            span,
            std::mem::take(ops),
            FunctionTerminator::Branch {
                target: first_target,
                span,
            },
        );

        for (index, ((arm, arm_target), check_block)) in if_pattern
            .arms
            .iter()
            .zip(arm_targets.iter())
            .zip(check_blocks.iter())
            .enumerate()
        {
            let mut check_ops = Vec::new();
            let mut check_current = *check_block;
            let mut condition_context = PatternConditionContext {
                scope,
                current: &mut check_current,
                ops: &mut check_ops,
                blocks,
                bool_ty: if_pattern.bool_ty,
            };
            let cond = self
                .pattern_condition(&target, &arm.pattern, &mut condition_context)
                .unwrap_or(FunctionExpr {
                    span: arm.pattern.span,
                    ty: if_pattern.bool_ty,
                    kind: FunctionExprKind::Bool(true),
                });
            let next_target = check_blocks.get(index + 1).copied().unwrap_or(else_target);
            self.finish_block(
                blocks,
                check_current,
                scope,
                arm.pattern.span,
                check_ops,
                FunctionTerminator::If {
                    cond,
                    then_target: *arm_target,
                    else_target: next_target,
                    span: arm.pattern.span,
                },
            );
        }

        for (arm, arm_target) in if_pattern.arms.iter().zip(arm_targets) {
            let arm_scope = self.alloc_scope(Some(scope), arm.span);
            let mut arm_ops = Vec::new();
            self.lower_pattern_binding(&arm.pattern, &target, &mut arm_ops);
            if arm_ops.is_empty() {
                self.lower_body_into(
                    &arm.body,
                    arm_target,
                    arm_scope,
                    blocks,
                    Fallthrough::Branch(merge_target),
                );
            } else {
                let body_entry = self.alloc_block();
                self.finish_block(
                    blocks,
                    arm_target,
                    arm_scope,
                    arm.span,
                    arm_ops,
                    FunctionTerminator::Branch {
                        target: body_entry,
                        span: arm.span,
                    },
                );
                self.lower_body_into(
                    &arm.body,
                    body_entry,
                    arm_scope,
                    blocks,
                    Fallthrough::Branch(merge_target),
                );
            }
        }

        if let Some(else_branch) = &if_pattern.else_branch {
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
        if self.switch_requires_pattern_chain(switch) {
            let mut context = LoweringContext {
                scope,
                current,
                ops,
                blocks,
            };
            self.lower_switch_as_chain(span, switch, &mut context);
            return;
        }
        let target = self.lower_value_expr(&switch.target, scope, current, ops, blocks);
        let merge_target = self.alloc_block();
        let mut arms = Vec::new();
        let mut lowered_arms = Vec::new();
        let mut default = None;
        for arm in &switch.arms {
            let arm_target = self.alloc_block();
            for pattern in &arm.patterns {
                match &pattern.kind {
                    TypedSwitchPatternKind::Expr(pattern) => arms.push(FunctionSwitchArm {
                        pattern: self.lower_value_expr(pattern, scope, current, ops, blocks),
                        target: arm_target,
                    }),
                    TypedSwitchPatternKind::CheckedInt { value } => arms.push(FunctionSwitchArm {
                        pattern: self.checked_int_pattern_expr(*value, pattern.ty, pattern.span),
                        target: arm_target,
                    }),
                    TypedSwitchPatternKind::Wildcard => default = Some(arm_target),
                    TypedSwitchPatternKind::Range { .. }
                    | TypedSwitchPatternKind::CheckedIntRange { .. } => {}
                }
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
            let mut arm_context = SwitchStmtArmContext {
                span: arm.span,
                scope: arm_scope,
                entry: arm_target,
                merge_target,
                blocks,
            };
            self.lower_switch_arm_body(&arm.body, &mut arm_context);
        }

        *current = merge_target;
    }

    fn lower_switch_as_chain(
        &mut self,
        span: Span,
        switch: &TypedSwitch,
        context: &mut LoweringContext<'_>,
    ) {
        let target_value = self.lower_value_expr(
            &switch.target,
            context.scope,
            context.current,
            context.ops,
            context.blocks,
        );
        let target_local = self.alloc_temp_local(switch.target.span, switch.target.ty);
        context.ops.push(FunctionOp::StoreLocal {
            local_id: target_local,
            value: target_value,
            span: switch.target.span,
        });
        let target = FunctionExpr {
            span: switch.target.span,
            ty: switch.target.ty,
            kind: FunctionExprKind::Local(target_local),
        };
        let merge_target = self.alloc_block();
        let mut lowered_arms = Vec::new();
        let mut tests = Vec::new();
        let mut default = merge_target;
        for arm in &switch.arms {
            let arm_target = self.alloc_block();
            for pattern in &arm.patterns {
                if matches!(&pattern.kind, TypedSwitchPatternKind::Wildcard) {
                    default = arm_target;
                } else {
                    tests.push((pattern, arm_target));
                }
            }
            lowered_arms.push((arm_target, arm));
        }
        let check_blocks = tests.iter().map(|_| self.alloc_block()).collect::<Vec<_>>();
        let first_target = check_blocks.first().copied().unwrap_or(default);
        self.finish_block(
            context.blocks,
            *context.current,
            context.scope,
            span,
            std::mem::take(context.ops),
            FunctionTerminator::Branch {
                target: first_target,
                span,
            },
        );
        for (index, ((pattern, arm_target), check_block)) in
            tests.iter().zip(check_blocks.iter()).enumerate()
        {
            let mut check_ops = Vec::new();
            let mut check_current = *check_block;
            let mut condition_context = PatternConditionContext {
                scope: context.scope,
                current: &mut check_current,
                ops: &mut check_ops,
                blocks: context.blocks,
                bool_ty: switch.bool_ty,
            };
            let cond = self
                .switch_pattern_condition(&target, pattern, &mut condition_context)
                .unwrap_or(FunctionExpr {
                    span,
                    ty: switch.bool_ty,
                    kind: FunctionExprKind::Bool(true),
                });
            let else_target = check_blocks.get(index + 1).copied().unwrap_or(default);
            self.finish_block(
                context.blocks,
                check_current,
                context.scope,
                span,
                check_ops,
                FunctionTerminator::If {
                    cond,
                    then_target: *arm_target,
                    else_target,
                    span,
                },
            );
        }
        for (arm_target, arm) in lowered_arms {
            let arm_scope = self.alloc_scope(Some(context.scope), arm.span);
            let mut arm_context = SwitchStmtArmContext {
                span: arm.span,
                scope: arm_scope,
                entry: arm_target,
                merge_target,
                blocks: context.blocks,
            };
            self.lower_switch_arm_body(&arm.body, &mut arm_context);
        }
        *context.current = merge_target;
    }

    pub(super) fn lower_switch_arm_body(
        &mut self,
        body: &TypedSwitchArmBody,
        context: &mut SwitchStmtArmContext<'_>,
    ) {
        match body {
            TypedSwitchArmBody::Expr(expr) => {
                let mut current = context.entry;
                let mut ops = Vec::new();
                self.lower_expr_stmt(
                    context.span,
                    expr,
                    context.scope,
                    &mut current,
                    &mut ops,
                    context.blocks,
                );
                self.finish_block(
                    context.blocks,
                    current,
                    context.scope,
                    context.span,
                    ops,
                    FunctionTerminator::Branch {
                        target: context.merge_target,
                        span: context.span,
                    },
                );
            }
            TypedSwitchArmBody::Stmt(stmt) => {
                let mut current = context.entry;
                let mut ops = Vec::new();
                if !self.lower_stmt_into(
                    stmt,
                    context.scope,
                    &mut current,
                    &mut ops,
                    context.blocks,
                ) {
                    self.finish_block(
                        context.blocks,
                        current,
                        context.scope,
                        context.span,
                        ops,
                        FunctionTerminator::Branch {
                            target: context.merge_target,
                            span: context.span,
                        },
                    );
                }
            }
            TypedSwitchArmBody::Block(body) => {
                self.lower_body_into(
                    body,
                    context.entry,
                    context.scope,
                    context.blocks,
                    Fallthrough::Branch(context.merge_target),
                );
            }
        }
    }

    pub(super) fn lower_for_in_stmt(
        &mut self,
        span: Span,
        for_stmt: &TypedForIn,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) {
        let iter_value = self.lower_value_expr(&for_stmt.iter, scope, current, ops, blocks);
        let iter_local = self.alloc_temp_local(for_stmt.iter.span, for_stmt.iter.ty);
        ops.push(FunctionOp::Binding(FunctionBinding {
            local_id: iter_local,
            name: "__for_iter".to_string(),
            ty: for_stmt.iter.ty,
            value: Some(iter_value),
            is_let: false,
        }));

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
        let header_current = loop_header;
        let optional_item_ty = self.optional_ty(for_stmt.item_ty);
        let next_local = self.alloc_temp_local(span, optional_item_ty);
        let next_value =
            self.iterator_next_expr(span, iter_local, for_stmt.iter.ty, optional_item_ty);
        header_ops.push(FunctionOp::Binding(FunctionBinding {
            local_id: next_local,
            name: "__for_next".to_string(),
            ty: next_value.ty,
            value: Some(next_value),
            is_let: true,
        }));
        let next_expr = FunctionExpr {
            span,
            ty: optional_item_ty,
            kind: FunctionExprKind::Local(next_local),
        };
        let header = FunctionForHeader::Condition(self.optional_some_condition(span, &next_expr));
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
        let mut body_ops = Vec::new();
        if let Some(binding) = &for_stmt.binding {
            let item_value = FunctionExpr {
                span,
                ty: for_stmt.item_ty,
                kind: FunctionExprKind::TaggedUnionPayload {
                    expr: Box::new(next_expr),
                },
            };
            let binding_value = self.lower_binding_pattern_value(
                for_stmt.pattern_kind,
                for_stmt.binding_ty,
                item_value,
            );
            body_ops.push(FunctionOp::Binding(FunctionBinding {
                local_id: binding.local_id,
                name: binding.name.clone(),
                ty: for_stmt.binding_ty,
                value: Some(binding_value),
                is_let: true,
            }));
        }
        let body_scope = self.alloc_scope(Some(scope), for_stmt.body.span);
        self.lower_body_into_with_ops(
            &for_stmt.body,
            body_entry,
            body_scope,
            blocks,
            Fallthrough::Branch(continue_target),
            body_ops,
        );
        self.loop_targets.pop();

        self.finish_block(
            blocks,
            continue_target,
            scope,
            span,
            Vec::new(),
            FunctionTerminator::Branch {
                target: loop_header,
                span,
            },
        );
        *current = break_target;
    }

    fn optional_ty(&mut self, elem: nia_ids::InternedTyId) -> nia_ids::InternedTyId {
        self.interner
            .as_mut()
            .map(|interner| interner.intern(TyKind::Optional { elem }))
            .unwrap_or(elem)
    }

    fn iterator_next_expr(
        &self,
        span: Span,
        iter_local: nia_ids::LocalId,
        iter_ty: nia_ids::InternedTyId,
        optional_item_ty: nia_ids::InternedTyId,
    ) -> FunctionExpr {
        FunctionExpr {
            span,
            ty: optional_item_ty,
            kind: FunctionExprKind::Call {
                callee: FunctionCallee::BuiltinPlaceMethod {
                    trait_id: BuiltinTrait::Iterator,
                    method: BuiltinTraitMethod::IteratorNext,
                    self_ty: iter_ty,
                    trait_args: Vec::new(),
                    receiver: Box::new(FunctionExpr {
                        span,
                        ty: iter_ty,
                        kind: FunctionExprKind::Local(iter_local),
                    }),
                },
                args: Vec::new(),
            },
        }
    }

    fn optional_some_condition(&self, span: Span, value: &FunctionExpr) -> FunctionExpr {
        self.tagged_union_tag_condition(
            value,
            span,
            self.bool_ty(value.ty),
            FunctionOptionalTag::Some.discriminant(),
        )
    }

    pub(super) fn lower_while_stmt(
        &mut self,
        span: Span,
        while_stmt: &TypedWhile,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) {
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
        let header = FunctionForHeader::Condition(self.lower_value_expr(
            &while_stmt.cond,
            scope,
            &mut header_current,
            &mut header_ops,
            blocks,
        ));
        let body_entry = self.alloc_block();
        let continue_target = loop_header;
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
        let body_scope = self.alloc_scope(Some(scope), while_stmt.body.span);
        self.lower_body_into(
            &while_stmt.body,
            body_entry,
            body_scope,
            blocks,
            Fallthrough::Branch(continue_target),
        );
        self.loop_targets.pop();

        *current = break_target;
    }

    pub(super) fn lower_loop_stmt(
        &mut self,
        span: Span,
        loop_stmt: &TypedLoop,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) {
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

        let body_entry = self.alloc_block();
        let continue_target = loop_header;
        let break_target = self.alloc_block();
        self.finish_block(
            blocks,
            loop_header,
            scope,
            span,
            Vec::new(),
            FunctionTerminator::Loop {
                header: FunctionForHeader::Infinite,
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
        let body_scope = self.alloc_scope(Some(scope), loop_stmt.body.span);
        self.lower_body_into(
            &loop_stmt.body,
            body_entry,
            body_scope,
            blocks,
            Fallthrough::Branch(continue_target),
        );
        self.loop_targets.pop();

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
            Fallthrough::Tail => {
                let mut tail_is_terminating_effect = false;
                let value = body.tail.as_ref().and_then(|tail| {
                    if self.expr_lowers_as_terminating_effect(tail) {
                        self.lower_effect_expr(tail, scope, &mut current, &mut ops, blocks);
                        tail_is_terminating_effect = true;
                        None
                    } else if self.expr_lowers_as_effect_only(tail) {
                        self.lower_effect_expr(tail, scope, &mut current, &mut ops, blocks);
                        None
                    } else {
                        Some(self.lower_value_expr(tail, scope, &mut current, &mut ops, blocks))
                    }
                });
                if tail_is_terminating_effect {
                    FunctionTerminator::Error { span }
                } else {
                    FunctionTerminator::Tail { value, span }
                }
            }
            Fallthrough::Branch(target) => {
                if let Some(tail) = &body.tail {
                    self.lower_effect_expr(tail, scope, &mut current, &mut ops, blocks);
                }
                FunctionTerminator::Branch { target, span }
            }
            Fallthrough::StoreThenBranch { local_id, target } => {
                let mut tail_terminated = false;
                if let Some(tail) = &body.tail {
                    tail_terminated = self.store_expr_result_or_effect(
                        tail,
                        local_id,
                        scope,
                        &mut current,
                        &mut ops,
                        blocks,
                    );
                }
                if tail_terminated {
                    return;
                }
                FunctionTerminator::Branch { target, span }
            }
        };
        self.finish_block(blocks, current, scope, span, ops, terminator);
    }

    fn bool_ty(&self, ty: InternedTyId) -> InternedTyId {
        InternedTyId::new(ty.interner_id, TyInternerIndex::from_interner_index(15))
    }
}
