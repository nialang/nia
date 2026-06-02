// SPDX-License-Identifier: GPL-3.0-or-later
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
            TypedExprKind::Range(range) => {
                FunctionExprKind::Range(self.lower_range(range, scope, current, ops, blocks))
            }
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
            TypedExprKind::Unary { op, expr: inner }
                if matches!(op, UnaryOp::Ref | UnaryOp::RefConst)
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
            TypedExprKind::BuiltinValue(value) => {
                FunctionExprKind::BuiltinValue(Self::lower_builtin_value(value))
            }
        };
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
            kind,
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
            TypedExprKind::Switch(switch) => {
                self.lower_switch_expr_stmt(expr.span, switch, scope, current, ops, blocks);
            }
            _ => {
                let expr = self.lower_value_expr(expr, scope, current, ops, blocks);
                ops.push(FunctionOp::Expr(expr));
            }
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

    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_value_if_expr(
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

    pub(super) fn lower_value_switch_expr(
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
    pub(super) fn lower_value_switch_arm_body(
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
            TypedCallee::TraitMethod {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
                receiver,
            } => FunctionCallee::TraitMethod {
                trait_id: *trait_id,
                method_id: *method_id,
                method_name: method_name.clone(),
                self_ty: *self_ty,
                trait_args: trait_args.clone(),
                args: args.clone(),
                receiver: Box::new(self.lower_value_expr(receiver, scope, current, ops, blocks)),
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
                        nia_body_ir::BuiltinOperatorOp::Unary(op) => {
                            FunctionBuiltinOperatorOp::Unary(op)
                        }
                        nia_body_ir::BuiltinOperatorOp::Binary(op) => {
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
            BuiltinMethod::RangeIter => FunctionBuiltinMethod::RangeIter,
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
