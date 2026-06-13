// SPDX-License-Identifier: GPL-3.0-or-later
use super::support::{LoweringContext, PatternConditionContext, SwitchValueArmContext};
use super::*;

impl FunctionLowerer {
    pub(super) fn lower_value_expr(
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
                let mut context = LoweringContext {
                    scope,
                    current,
                    ops,
                    blocks,
                };
                return self.lower_value_if_expr(
                    expr,
                    cond,
                    then_branch,
                    else_branch.as_deref(),
                    &mut context,
                );
            }
            TypedExprKind::IfPattern(if_pattern) => {
                let mut context = LoweringContext {
                    scope,
                    current,
                    ops,
                    blocks,
                };
                return self.lower_value_if_pattern_expr(expr, if_pattern, &mut context);
            }
            TypedExprKind::Switch(switch) => {
                return self.lower_value_switch_expr(expr, switch, scope, current, ops, blocks);
            }
            TypedExprKind::Range(range) => {
                FunctionExprKind::Range(self.lower_range(range, scope, current, ops, blocks))
            }
            TypedExprKind::MemoryIntrinsic(memory) => {
                let op = self.lower_memory_intrinsic_op(memory, scope, current, ops, blocks);
                ops.push(op);
                FunctionExprKind::Error
            }
            TypedExprKind::Atomic(atomic) => {
                FunctionExprKind::Atomic(self.lower_atomic(atomic, scope, current, ops, blocks))
            }
            TypedExprKind::LoadUnaligned { ty, ptr } => FunctionExprKind::LoadUnaligned {
                ty: *ty,
                ptr: Box::new(self.lower_value_expr(ptr, scope, current, ops, blocks)),
            },
            TypedExprKind::Splat { value } => FunctionExprKind::Splat {
                value: Box::new(self.lower_value_expr(value, scope, current, ops, blocks)),
            },
            TypedExprKind::ExtractElement { vector, index } => FunctionExprKind::ExtractElement {
                vector: Box::new(self.lower_value_expr(vector, scope, current, ops, blocks)),
                index: Box::new(self.lower_value_expr(index, scope, current, ops, blocks)),
            },
            TypedExprKind::InsertElement {
                vector,
                index,
                value,
            } => FunctionExprKind::InsertElement {
                vector: Box::new(self.lower_value_expr(vector, scope, current, ops, blocks)),
                index: Box::new(self.lower_value_expr(index, scope, current, ops, blocks)),
                value: Box::new(self.lower_value_expr(value, scope, current, ops, blocks)),
            },
            TypedExprKind::Bitmask { vector } => FunctionExprKind::Bitmask {
                vector: Box::new(self.lower_value_expr(vector, scope, current, ops, blocks)),
            },
            TypedExprKind::BitIntrinsic { op, value } => FunctionExprKind::BitIntrinsic {
                op: self.lower_bit_intrinsic_op(*op),
                value: Box::new(self.lower_value_expr(value, scope, current, ops, blocks)),
            },
            TypedExprKind::CStringPointer { array, is_readonly } => {
                FunctionExprKind::CStringPointer {
                    array: Box::new(self.lower_value_expr(array, scope, current, ops, blocks)),
                    is_readonly: *is_readonly,
                }
            }
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
            TypedExprKind::Unary { op, expr: inner }
                if matches!(op, UnaryOp::Ref | UnaryOp::RefReadOnly)
                    && matches!(inner.kind, TypedExprKind::Slice { .. }) =>
            {
                return self.lower_value_expr(inner, scope, current, ops, blocks);
            }
            TypedExprKind::Unary { op, expr: inner }
                if matches!(op, UnaryOp::Ref | UnaryOp::RefReadOnly)
                    && !matches!(
                        inner.kind,
                        TypedExprKind::Function(_) | TypedExprKind::FunctionInstance { .. }
                    ) =>
            {
                FunctionExprKind::AddrOf(self.lower_expr_place(inner, scope, current, ops, blocks))
            }
            TypedExprKind::Unary { op, expr: inner } => FunctionExprKind::Unary {
                op: *op,
                expr: Box::new(self.lower_value_expr(inner, scope, current, ops, blocks)),
            },
            TypedExprKind::OptionalSome { expr: inner } => FunctionExprKind::OptionalSome {
                expr: Box::new(self.lower_value_expr(inner, scope, current, ops, blocks)),
            },
            TypedExprKind::ErrorOk { expr: inner } => FunctionExprKind::ErrorOk {
                expr: Box::new(self.lower_value_expr(inner, scope, current, ops, blocks)),
            },
            TypedExprKind::ErrorErr { expr: inner } => FunctionExprKind::ErrorErr {
                expr: Box::new(self.lower_value_expr(inner, scope, current, ops, blocks)),
            },
            TypedExprKind::Try { expr: inner } => {
                return self.lower_try_expr(expr, inner, scope, current, ops, blocks);
            }
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
            TypedExprKind::TraitObjectUpcast {
                expr: inner,
                source_ty,
                target_ty,
            } => FunctionExprKind::TraitObjectUpcast {
                expr: Box::new(self.lower_value_expr(inner, scope, current, ops, blocks)),
                source_ty: *source_ty,
                target_ty: *target_ty,
            },
            TypedExprKind::TraitObjectCoercion {
                expr: inner,
                target_ty,
                self_ty,
            } => FunctionExprKind::TraitObjectCoercion {
                expr: Box::new(self.lower_value_expr(inner, scope, current, ops, blocks)),
                target_ty: *target_ty,
                self_ty: *self_ty,
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
                is_readonly,
            } => FunctionExprKind::Slice {
                lhs: Box::new(self.lower_value_expr(lhs, scope, current, ops, blocks)),
                range: self.lower_slice_range(range, scope, current, ops, blocks),
                is_readonly: *is_readonly,
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
            TypedExprKind::Null => FunctionExprKind::Null,
            TypedExprKind::Local(local_id) => FunctionExprKind::Local(*local_id),
            TypedExprKind::Global(def_id) => FunctionExprKind::Global(*def_id),
            TypedExprKind::Function(def_id) => FunctionExprKind::Function(*def_id),
            TypedExprKind::FunctionInstance {
                def_id,
                arg_module_id,
                args,
            } => FunctionExprKind::FunctionInstance {
                def_id: *def_id,
                arg_module_id: *arg_module_id,
                args: args.clone(),
            },
            TypedExprKind::EnumVariant(def_id) => FunctionExprKind::EnumVariant(*def_id),
            TypedExprKind::BuiltinValue(value) => {
                FunctionExprKind::BuiltinValue(Self::lower_builtin_value(value))
            }
            TypedExprKind::Trap => FunctionExprKind::Trap,
        };
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
            kind,
        }
    }

    fn lower_try_expr(
        &mut self,
        expr: &TypedExpr,
        inner: &TypedExpr,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionExpr {
        let value = self.lower_value_expr(inner, scope, current, ops, blocks);
        let local = self.alloc_temp_local(expr.span, expr.ty);
        let success_target = self.alloc_block();
        let kind = self.try_kind(inner.ty).unwrap_or(FunctionTryKind::Optional);
        self.finish_block(
            blocks,
            *current,
            scope,
            expr.span,
            std::mem::take(ops),
            FunctionTerminator::Try {
                value,
                kind,
                success_local: local,
                success_target,
                span: expr.span,
            },
        );
        *current = success_target;
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
            kind: FunctionExprKind::Local(local),
        }
    }

    pub(super) fn lower_defer_expr(&mut self, expr: &TypedExpr) -> FunctionDeferBody {
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

    pub(super) fn lower_effect_expr(
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
                StatementIf {
                    cond,
                    then_branch,
                    else_branch: else_branch.as_deref(),
                },
                scope,
                current,
                ops,
                blocks,
            ),
            TypedExprKind::IfPattern(if_pattern) => {
                self.lower_if_pattern_expr_stmt(expr.span, if_pattern, scope, current, ops, blocks);
            }
            TypedExprKind::Switch(switch) => {
                self.lower_switch_expr_stmt(expr.span, switch, scope, current, ops, blocks);
            }
            TypedExprKind::MemoryIntrinsic(memory) => {
                let op = self.lower_memory_intrinsic_op(memory, scope, current, ops, blocks);
                ops.push(op);
            }
            TypedExprKind::Atomic(atomic) => {
                let expr = FunctionExpr {
                    span: expr.span,
                    ty: expr.ty,
                    kind: FunctionExprKind::Atomic(
                        self.lower_atomic(atomic, scope, current, ops, blocks),
                    ),
                };
                ops.push(FunctionOp::Expr(expr));
            }
            _ => {
                let expr = self.lower_value_expr(expr, scope, current, ops, blocks);
                ops.push(FunctionOp::Expr(expr));
            }
        }
    }

    pub(super) fn lower_memory_intrinsic_op(
        &mut self,
        memory: &nia_body_ir::TypedMemoryIntrinsic,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionOp {
        let op = match memory.op {
            nia_body_ir::MemoryIntrinsicOp::Copy => FunctionMemoryIntrinsicOp::Copy,
            nia_body_ir::MemoryIntrinsicOp::Move => FunctionMemoryIntrinsicOp::Move,
            nia_body_ir::MemoryIntrinsicOp::Set => FunctionMemoryIntrinsicOp::Set,
        };
        let dest = self.lower_value_expr(&memory.dest, scope, current, ops, blocks);
        let source = match &memory.source {
            nia_body_ir::TypedMemoryIntrinsicSource::Slice(source) => {
                FunctionMemoryIntrinsicSource::Slice(
                    self.lower_value_expr(source, scope, current, ops, blocks),
                )
            }
            nia_body_ir::TypedMemoryIntrinsicSource::Byte(value) => {
                FunctionMemoryIntrinsicSource::Byte(
                    self.lower_value_expr(value, scope, current, ops, blocks),
                )
            }
        };
        FunctionOp::MemoryIntrinsic(FunctionMemoryIntrinsic {
            span: memory.dest.span,
            op,
            elem_ty: memory.elem_ty,
            dest,
            source,
        })
    }

    fn lower_atomic(
        &mut self,
        atomic: &TypedAtomic,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionAtomic {
        match atomic {
            TypedAtomic::Load { ty, ptr, order } => FunctionAtomic::Load {
                ty: *ty,
                ptr: Box::new(self.lower_value_expr(ptr, scope, current, ops, blocks)),
                order: lower_atomic_order(*order),
            },
            TypedAtomic::Store {
                ty,
                ptr,
                value,
                order,
            } => FunctionAtomic::Store {
                ty: *ty,
                ptr: Box::new(self.lower_value_expr(ptr, scope, current, ops, blocks)),
                value: Box::new(self.lower_value_expr(value, scope, current, ops, blocks)),
                order: lower_atomic_order(*order),
            },
            TypedAtomic::Rmw {
                ty,
                ptr,
                op,
                value,
                order,
            } => FunctionAtomic::Rmw {
                ty: *ty,
                ptr: Box::new(self.lower_value_expr(ptr, scope, current, ops, blocks)),
                op: lower_atomic_rmw_op(*op),
                value: Box::new(self.lower_value_expr(value, scope, current, ops, blocks)),
                order: lower_atomic_order(*order),
            },
            TypedAtomic::Cmpxchg {
                ty,
                ptr,
                expected,
                desired,
                success,
                failure,
                weak,
            } => FunctionAtomic::Cmpxchg {
                ty: *ty,
                ptr: Box::new(self.lower_value_expr(ptr, scope, current, ops, blocks)),
                expected: Box::new(self.lower_value_expr(expected, scope, current, ops, blocks)),
                desired: Box::new(self.lower_value_expr(desired, scope, current, ops, blocks)),
                success: lower_atomic_order(*success),
                failure: lower_atomic_order(*failure),
                weak: *weak,
            },
            TypedAtomic::Fence { order } => FunctionAtomic::Fence {
                order: lower_atomic_order(*order),
            },
        }
    }

    pub(super) fn lower_value_block_expr(
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

    pub(super) fn lower_value_if_expr(
        &mut self,
        expr: &TypedExpr,
        cond: &TypedExpr,
        then_branch: &TypedBody,
        else_branch: Option<&TypedExpr>,
        context: &mut LoweringContext<'_>,
    ) -> FunctionExpr {
        let cond = self.lower_value_expr(
            cond,
            context.scope,
            context.current,
            context.ops,
            context.blocks,
        );
        let local = self.alloc_temp_local(expr.span, expr.ty);
        let then_target = self.alloc_block();
        let else_target = self.alloc_block();
        let merge_target = self.alloc_block();
        self.finish_block(
            context.blocks,
            *context.current,
            context.scope,
            expr.span,
            std::mem::take(context.ops),
            FunctionTerminator::If {
                cond,
                then_target,
                else_target,
                span: expr.span,
            },
        );

        let then_scope = self.alloc_scope(Some(context.scope), then_branch.span);
        self.lower_body_into(
            then_branch,
            then_target,
            then_scope,
            context.blocks,
            Fallthrough::StoreThenBranch {
                local_id: local,
                target: merge_target,
            },
        );

        let mut else_current = else_target;
        let mut else_ops = Vec::new();
        if let Some(else_branch) = else_branch {
            let value = self.lower_value_expr(
                else_branch,
                context.scope,
                &mut else_current,
                &mut else_ops,
                context.blocks,
            );
            else_ops.push(FunctionOp::StoreLocal {
                local_id: local,
                value,
                span: else_branch.span,
            });
        }
        self.finish_block(
            context.blocks,
            else_current,
            context.scope,
            else_branch.map(|expr| expr.span).unwrap_or(expr.span),
            else_ops,
            FunctionTerminator::Branch {
                target: merge_target,
                span: else_branch.map(|expr| expr.span).unwrap_or(expr.span),
            },
        );

        *context.current = merge_target;
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
            kind: FunctionExprKind::Local(local),
        }
    }

    pub(super) fn lower_value_if_pattern_expr(
        &mut self,
        expr: &TypedExpr,
        if_pattern: &TypedIfPattern,
        context: &mut LoweringContext<'_>,
    ) -> FunctionExpr {
        let target_value = self.lower_value_expr(
            &if_pattern.target,
            context.scope,
            context.current,
            context.ops,
            context.blocks,
        );
        let target_local = self.alloc_temp_local(if_pattern.target.span, if_pattern.target.ty);
        context.ops.push(FunctionOp::StoreLocal {
            local_id: target_local,
            value: target_value,
            span: if_pattern.target.span,
        });
        let target = FunctionExpr {
            span: if_pattern.target.span,
            ty: if_pattern.target.ty,
            kind: FunctionExprKind::Local(target_local),
        };
        let result_local = self.alloc_temp_local(expr.span, expr.ty);
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
            context.blocks,
            *context.current,
            context.scope,
            expr.span,
            std::mem::take(context.ops),
            FunctionTerminator::Branch {
                target: first_target,
                span: expr.span,
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
                scope: context.scope,
                current: &mut check_current,
                ops: &mut check_ops,
                blocks: context.blocks,
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
                context.blocks,
                check_current,
                context.scope,
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
            let arm_scope = self.alloc_scope(Some(context.scope), arm.span);
            let mut ops = Vec::new();
            self.lower_pattern_binding(&arm.pattern, &target, &mut ops);
            if ops.is_empty() {
                self.lower_body_into(
                    &arm.body,
                    arm_target,
                    arm_scope,
                    context.blocks,
                    Fallthrough::StoreThenBranch {
                        local_id: result_local,
                        target: merge_target,
                    },
                );
            } else {
                let body_entry = self.alloc_block();
                self.finish_block(
                    context.blocks,
                    arm_target,
                    arm_scope,
                    arm.span,
                    ops,
                    FunctionTerminator::Branch {
                        target: body_entry,
                        span: arm.span,
                    },
                );
                self.lower_body_into(
                    &arm.body,
                    body_entry,
                    arm_scope,
                    context.blocks,
                    Fallthrough::StoreThenBranch {
                        local_id: result_local,
                        target: merge_target,
                    },
                );
            }
        }

        if let Some(else_branch) = &if_pattern.else_branch {
            let mut else_current = else_target;
            let mut else_ops = Vec::new();
            let value = self.lower_value_expr(
                else_branch,
                context.scope,
                &mut else_current,
                &mut else_ops,
                context.blocks,
            );
            else_ops.push(FunctionOp::StoreLocal {
                local_id: result_local,
                value,
                span: else_branch.span,
            });
            self.finish_block(
                context.blocks,
                else_current,
                context.scope,
                else_branch.span,
                else_ops,
                FunctionTerminator::Branch {
                    target: merge_target,
                    span: else_branch.span,
                },
            );
        }

        *context.current = merge_target;
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
            kind: FunctionExprKind::Local(result_local),
        }
    }

    pub(super) fn lower_value_switch_expr(
        &mut self,
        expr: &TypedExpr,
        switch: &TypedSwitch,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionExpr {
        if self.switch_requires_pattern_chain(switch) {
            let mut context = LoweringContext {
                scope,
                current,
                ops,
                blocks,
            };
            return self.lower_value_switch_expr_as_chain(expr, switch, &mut context);
        }
        let target = self.lower_value_expr(&switch.target, scope, current, ops, blocks);
        let arm_target_value = target.clone();
        let local = self.alloc_temp_local(expr.span, expr.ty);
        let merge_target = self.alloc_block();
        let mut arms = Vec::new();
        let mut lowered_arms = Vec::new();
        let mut default = None;
        for arm in &switch.arms {
            let arm_target = self.alloc_block();
            for pattern in &arm.patterns {
                match &pattern.kind {
                    TypedPatternKind::Expr(pattern) => arms.push(FunctionSwitchArm {
                        pattern: self.lower_value_expr(pattern, scope, current, ops, blocks),
                        target: arm_target,
                    }),
                    TypedPatternKind::CheckedInt { value } => arms.push(FunctionSwitchArm {
                        pattern: self.checked_int_pattern_expr(*value, pattern.ty, pattern.span),
                        target: arm_target,
                    }),
                    TypedPatternKind::Wildcard => default = Some(arm_target),
                    TypedPatternKind::Bind { .. }
                    | TypedPatternKind::OptionalSome(_)
                    | TypedPatternKind::OptionalNull
                    | TypedPatternKind::ErrorOk(_)
                    | TypedPatternKind::ErrorErr(_)
                    | TypedPatternKind::Range { .. }
                    | TypedPatternKind::CheckedIntRange { .. } => {}
                }
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
            let mut arm_context = SwitchValueArmContext {
                span: arm.span,
                scope: arm_scope,
                entry: arm_target,
                result_local: local,
                merge_target,
                blocks,
                patterns: &arm.patterns,
                target: &arm_target_value,
            };
            self.lower_value_switch_arm_body(&arm.body, &mut arm_context);
        }

        *current = merge_target;
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
            kind: FunctionExprKind::Local(local),
        }
    }

    fn lower_value_switch_expr_as_chain(
        &mut self,
        expr: &TypedExpr,
        switch: &TypedSwitch,
        context: &mut LoweringContext<'_>,
    ) -> FunctionExpr {
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
        let result_local = self.alloc_temp_local(expr.span, expr.ty);
        let merge_target = self.alloc_block();
        let mut lowered_arms = Vec::new();
        let mut tests = Vec::new();
        let mut default = merge_target;
        for arm in &switch.arms {
            let arm_target = self.alloc_block();
            for pattern in &arm.patterns {
                if matches!(&pattern.kind, TypedPatternKind::Wildcard) {
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
            expr.span,
            std::mem::take(context.ops),
            FunctionTerminator::Branch {
                target: first_target,
                span: expr.span,
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
                .pattern_condition(&target, pattern, &mut condition_context)
                .unwrap_or(FunctionExpr {
                    span: expr.span,
                    ty: switch.bool_ty,
                    kind: FunctionExprKind::Bool(true),
                });
            let else_target = check_blocks.get(index + 1).copied().unwrap_or(default);
            self.finish_block(
                context.blocks,
                check_current,
                context.scope,
                expr.span,
                check_ops,
                FunctionTerminator::If {
                    cond,
                    then_target: *arm_target,
                    else_target,
                    span: expr.span,
                },
            );
        }
        for (arm_target, arm) in lowered_arms {
            let arm_scope = self.alloc_scope(Some(context.scope), arm.span);
            let mut arm_context = SwitchValueArmContext {
                span: arm.span,
                scope: arm_scope,
                entry: arm_target,
                result_local,
                merge_target,
                blocks: context.blocks,
                patterns: &arm.patterns,
                target: &target,
            };
            self.lower_value_switch_arm_body(&arm.body, &mut arm_context);
        }
        *context.current = merge_target;
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
            kind: FunctionExprKind::Local(result_local),
        }
    }

    pub(super) fn lower_value_switch_arm_body(
        &mut self,
        body: &TypedSwitchArmBody,
        context: &mut SwitchValueArmContext<'_>,
    ) {
        match body {
            TypedSwitchArmBody::Expr(expr) => {
                let mut current = context.entry;
                let mut ops = Vec::new();
                self.lower_pattern_bindings(context.patterns, context.target, &mut ops);
                let value = self.lower_value_expr(
                    expr,
                    context.scope,
                    &mut current,
                    &mut ops,
                    context.blocks,
                );
                ops.push(FunctionOp::StoreLocal {
                    local_id: context.result_local,
                    value,
                    span: expr.span,
                });
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
                self.lower_pattern_bindings(context.patterns, context.target, &mut ops);
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
                let mut ops = Vec::new();
                self.lower_pattern_bindings(context.patterns, context.target, &mut ops);
                if ops.is_empty() {
                    self.lower_body_into(
                        body,
                        context.entry,
                        context.scope,
                        context.blocks,
                        Fallthrough::StoreThenBranch {
                            local_id: context.result_local,
                            target: context.merge_target,
                        },
                    );
                } else {
                    let body_entry = self.alloc_block();
                    self.finish_block(
                        context.blocks,
                        context.entry,
                        context.scope,
                        context.span,
                        ops,
                        FunctionTerminator::Branch {
                            target: body_entry,
                            span: context.span,
                        },
                    );
                    self.lower_body_into(
                        body,
                        body_entry,
                        context.scope,
                        context.blocks,
                        Fallthrough::StoreThenBranch {
                            local_id: context.result_local,
                            target: context.merge_target,
                        },
                    );
                }
            }
        }
    }

    pub(super) fn lower_array_elements(
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

    pub(super) fn lower_callee(
        &mut self,
        callee: &TypedCallee,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionCallee {
        match callee {
            TypedCallee::Function(def_id) => FunctionCallee::Function(*def_id),
            TypedCallee::FunctionInstance {
                def_id,
                arg_module_id,
                args,
            } => FunctionCallee::FunctionInstance {
                def_id: *def_id,
                arg_module_id: *arg_module_id,
                args: args.clone(),
            },
            TypedCallee::Method {
                def_id,
                args,
                receiver_kind,
                receiver,
            } => FunctionCallee::Method {
                def_id: *def_id,
                arg_module_id: self.module_id,
                args: args.clone(),
                receiver_kind: *receiver_kind,
                receiver: Box::new(self.lower_value_expr(receiver, scope, current, ops, blocks)),
            },
            TypedCallee::TraitMethod {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
                receiver_kind,
                receiver,
            } => FunctionCallee::TraitMethod {
                trait_id: *trait_id,
                method_id: *method_id,
                method_name: method_name.clone(),
                self_ty: *self_ty,
                trait_args: trait_args.clone(),
                args: args.clone(),
                receiver_kind: *receiver_kind,
                receiver: Box::new(self.lower_value_expr(receiver, scope, current, ops, blocks)),
            },
            TypedCallee::TraitAssociatedFunction {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
            } => FunctionCallee::TraitAssociatedFunction {
                trait_id: *trait_id,
                method_id: *method_id,
                method_name: method_name.clone(),
                self_ty: *self_ty,
                trait_args: trait_args.clone(),
                args: args.clone(),
            },
            TypedCallee::DynamicTraitMethod {
                object_ty,
                trait_id,
                method_id,
                method_name,
                trait_args,
                slot,
                params,
                return_type,
                receiver_kind,
                receiver,
            } => FunctionCallee::DynamicTraitMethod {
                object_ty: *object_ty,
                trait_id: *trait_id,
                method_id: *method_id,
                method_name: method_name.clone(),
                trait_args: trait_args.clone(),
                slot: *slot,
                params: params.clone(),
                return_type: *return_type,
                receiver_kind: *receiver_kind,
                receiver: Box::new(self.lower_value_expr(receiver, scope, current, ops, blocks)),
            },
            TypedCallee::BuiltinMethod {
                method,
                self_ty,
                receiver,
            } => FunctionCallee::BuiltinMethod {
                method: Self::lower_builtin_method(*method),
                self_ty: *self_ty,
                receiver: Box::new(self.lower_value_expr(receiver, scope, current, ops, blocks)),
            },
            TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod {
                trait_id,
                method,
                self_ty,
                trait_args,
                receiver,
            }) => FunctionCallee::BuiltinPlaceMethod {
                trait_id: *trait_id,
                method: *method,
                self_ty: *self_ty,
                trait_args: trait_args.clone(),
                receiver: Box::new(self.lower_value_expr(receiver, scope, current, ops, blocks)),
            },
            TypedCallee::BuiltinOperator(operator) => {
                FunctionCallee::BuiltinOperator(FunctionBuiltinOperator {
                    trait_id: operator.trait_id,
                    op: match operator.op {
                        nia_sema_ir::BuiltinOperatorOp::Unary(op) => {
                            FunctionBuiltinOperatorOp::Unary(op)
                        }
                        nia_sema_ir::BuiltinOperatorOp::Binary(op) => {
                            FunctionBuiltinOperatorOp::Binary(op)
                        }
                    },
                })
            }
            TypedCallee::FunctionPointer(expr) => FunctionCallee::FunctionPointer(Box::new(
                self.lower_value_expr(expr, scope, current, ops, blocks),
            )),
        }
    }

    fn lower_builtin_method(method: BuiltinMethod) -> FunctionBuiltinMethod {
        match method {
            BuiltinMethod::Len => FunctionBuiltinMethod::Len,
            BuiltinMethod::Start => FunctionBuiltinMethod::Start,
            BuiltinMethod::End => FunctionBuiltinMethod::End,
        }
    }

    pub(super) fn lower_slice_range(
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

    pub(super) fn lower_range(
        &mut self,
        range: &TypedRange,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionRange {
        FunctionRange {
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

    pub(super) fn lower_inline_asm(
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
            options: asm.options.iter().map(Self::lower_asm_option).collect(),
        }
    }
}

impl FunctionLowerer {
    fn lower_bit_intrinsic_op(
        &self,
        op: nia_body_ir::TypedBitIntrinsicOp,
    ) -> FunctionBitIntrinsicOp {
        match op {
            nia_body_ir::TypedBitIntrinsicOp::Ctz => FunctionBitIntrinsicOp::Ctz,
            nia_body_ir::TypedBitIntrinsicOp::Clz => FunctionBitIntrinsicOp::Clz,
            nia_body_ir::TypedBitIntrinsicOp::Popcount => FunctionBitIntrinsicOp::Popcount,
        }
    }
}

fn lower_atomic_order(order: nia_body_ir::AtomicOrder) -> AtomicOrder {
    match order {
        nia_body_ir::AtomicOrder::Unordered => AtomicOrder::Unordered,
        nia_body_ir::AtomicOrder::Monotonic => AtomicOrder::Monotonic,
        nia_body_ir::AtomicOrder::Acquire => AtomicOrder::Acquire,
        nia_body_ir::AtomicOrder::Release => AtomicOrder::Release,
        nia_body_ir::AtomicOrder::AcqRel => AtomicOrder::AcqRel,
        nia_body_ir::AtomicOrder::SeqCst => AtomicOrder::SeqCst,
    }
}

fn lower_atomic_rmw_op(op: nia_body_ir::AtomicRmwOp) -> AtomicRmwOp {
    match op {
        nia_body_ir::AtomicRmwOp::Xchg => AtomicRmwOp::Xchg,
        nia_body_ir::AtomicRmwOp::Add => AtomicRmwOp::Add,
        nia_body_ir::AtomicRmwOp::Sub => AtomicRmwOp::Sub,
        nia_body_ir::AtomicRmwOp::And => AtomicRmwOp::And,
        nia_body_ir::AtomicRmwOp::Nand => AtomicRmwOp::Nand,
        nia_body_ir::AtomicRmwOp::Or => AtomicRmwOp::Or,
        nia_body_ir::AtomicRmwOp::Xor => AtomicRmwOp::Xor,
        nia_body_ir::AtomicRmwOp::Max => AtomicRmwOp::Max,
        nia_body_ir::AtomicRmwOp::Min => AtomicRmwOp::Min,
        nia_body_ir::AtomicRmwOp::UMax => AtomicRmwOp::UMax,
        nia_body_ir::AtomicRmwOp::UMin => AtomicRmwOp::UMin,
    }
}
