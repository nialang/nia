// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::UnaryOp;
use nia_body_ir::{
    AsmOption, BuiltinConst, BuiltinPlaceMethod, PlaceBase, PlaceElem, TypedArrayElements,
    TypedBinding, TypedBody, TypedCallee, TypedExpr, TypedExprKind, TypedForHeader, TypedForInit,
    TypedInlineAsm, TypedLocal, TypedLocalKind, TypedPlace, TypedRange, TypedSliceRange, TypedStmt,
    TypedStmtKind, TypedSwitch, TypedSwitchArmBody, TypedSwitchPattern,
};
use nia_ids::{InternedTyId, LocalId};
use nia_span::Span;

use nia_function_ir::{
    FunctionArrayElements, FunctionAsmInput, FunctionAsmOption, FunctionAsmOutput, FunctionBinding,
    FunctionBlock, FunctionBlockId, FunctionBody, FunctionBuiltinOperator,
    FunctionBuiltinOperatorOp, FunctionBuiltinValue, FunctionCallee, FunctionDeferBody,
    FunctionExpr, FunctionExprKind, FunctionFieldInit, FunctionForHeader, FunctionInlineAsm,
    FunctionLocal, FunctionLocalKind, FunctionOp, FunctionPlace, FunctionPlaceBase,
    FunctionPlaceElem, FunctionRange, FunctionScope, FunctionScopeId, FunctionSliceRange,
    FunctionSwitchArm, FunctionTerminator,
};

#[cfg(test)]
mod tests;

pub fn lower_function_body(body: &TypedBody) -> FunctionBody {
    FunctionLowerer::new().lower_body(body)
}

struct FunctionLowerer {
    next_block: u32,
    next_scope: u32,
    next_temp_local: u32,
    temp_locals: Vec<FunctionLocal>,
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

struct StatementIf<'a> {
    cond: &'a TypedExpr,
    then_branch: &'a TypedBody,
    else_branch: Option<&'a TypedExpr>,
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

    fn lower_range(
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
            options: asm.options.iter().map(Self::lower_asm_option).collect(),
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
                PlaceBase::Error => FunctionPlaceBase::Error,
            },
            elems: place
                .elems
                .iter()
                .map(|elem| match elem {
                    PlaceElem::Field(field) => FunctionPlaceElem::Field(*field),
                    PlaceElem::Index(index) => FunctionPlaceElem::Index(Box::new(
                        self.lower_value_expr(index, scope, current, ops, blocks),
                    )),
                    PlaceElem::Error => FunctionPlaceElem::Error,
                })
                .collect(),
        }
    }

    fn lower_expr_place(
        &mut self,
        expr: &TypedExpr,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionPlace {
        match &expr.kind {
            TypedExprKind::Global(def_id) => FunctionPlace {
                span: expr.span,
                ty: expr.ty,
                base: FunctionPlaceBase::Global(*def_id),
                elems: Vec::new(),
            },
            TypedExprKind::Local(local_id) => FunctionPlace {
                span: expr.span,
                ty: expr.ty,
                base: FunctionPlaceBase::Local(*local_id),
                elems: Vec::new(),
            },
            TypedExprKind::Unary {
                op: UnaryOp::Deref,
                expr: inner,
            } => FunctionPlace {
                span: expr.span,
                ty: expr.ty,
                base: FunctionPlaceBase::Deref(Box::new(
                    self.lower_value_expr(inner, scope, current, ops, blocks),
                )),
                elems: Vec::new(),
            },
            TypedExprKind::Field { lhs, field } => {
                let mut place = self.lower_expr_place(lhs, scope, current, ops, blocks);
                place.span = expr.span;
                place.ty = expr.ty;
                place.elems.push(FunctionPlaceElem::Field(*field));
                place
            }
            TypedExprKind::Index { lhs, index } => {
                let mut place = self.lower_expr_place(lhs, scope, current, ops, blocks);
                place.span = expr.span;
                place.ty = expr.ty;
                place.elems.push(FunctionPlaceElem::Index(Box::new(
                    self.lower_value_expr(index, scope, current, ops, blocks),
                )));
                place
            }
            _ => unreachable!("address-of expression must be a body-checked place"),
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
        self.temp_locals.push(FunctionLocal {
            id,
            name: format!("fir.tmp.{}", id.0),
            kind: FunctionLocalKind::Binding,
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

    fn collect_body_locals(&self, body: &TypedBody, locals: &mut Vec<FunctionLocal>) {
        self.extend_unique_locals(&body.locals, locals);
        self.collect_nested_body_locals(body, locals);
    }

    fn collect_nested_body_locals(&self, body: &TypedBody, locals: &mut Vec<FunctionLocal>) {
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

    fn collect_expr_locals(&self, expr: &TypedExpr, locals: &mut Vec<FunctionLocal>) {
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

    fn extend_unique_locals(&self, source: &[TypedLocal], target: &mut Vec<FunctionLocal>) {
        for local in source {
            if !target.iter().any(|existing| existing.id == local.id) {
                target.push(Self::lower_local(local));
            }
        }
    }

    fn lower_local(local: &TypedLocal) -> FunctionLocal {
        FunctionLocal {
            id: local.id,
            name: local.name.clone(),
            kind: Self::lower_local_kind(local.kind),
            ty: local.ty,
            span: local.span,
        }
    }

    fn lower_local_kind(kind: TypedLocalKind) -> FunctionLocalKind {
        match kind {
            TypedLocalKind::Param => FunctionLocalKind::Param,
            TypedLocalKind::Binding => FunctionLocalKind::Binding,
            TypedLocalKind::ConstBinding => FunctionLocalKind::ConstBinding,
        }
    }

    fn lower_builtin_value(value: &BuiltinConst) -> FunctionBuiltinValue {
        match value {
            BuiltinConst::Usize(value) => FunctionBuiltinValue::Usize(*value),
            BuiltinConst::Layout { builtin, ty } => FunctionBuiltinValue::Layout {
                builtin: *builtin,
                ty: *ty,
            },
            BuiltinConst::Int(value) => FunctionBuiltinValue::Int(*value),
        }
    }

    fn lower_asm_option(option: &AsmOption) -> FunctionAsmOption {
        match option {
            AsmOption::Volatile => FunctionAsmOption::Volatile,
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
                TypedExprKind::CStringPointer { array: inner, .. }
                | TypedExprKind::Unary { expr: inner, .. }
                | TypedExprKind::Discard(inner)
                | TypedExprKind::Cast { expr: inner, .. } => visit_expr(inner, max_id),
                TypedExprKind::Range(range) => {
                    if let Some(start) = &range.start {
                        visit_expr(start, max_id);
                    }
                    if let Some(end) = &range.end {
                        visit_expr(end, max_id);
                    }
                }
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
                TypedCallee::Method { receiver, .. }
                | TypedCallee::TraitMethod { receiver, .. }
                | TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod { receiver, .. })
                | TypedCallee::FunctionPointer(receiver) => {
                    visit_expr(receiver, max_id);
                }
                TypedCallee::Function(_)
                | TypedCallee::FunctionInstance { .. }
                | TypedCallee::BuiltinOperator(_) => {}
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
