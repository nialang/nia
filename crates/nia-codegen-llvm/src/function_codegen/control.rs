// SPDX-License-Identifier: GPL-3.0-or-later
use nia_body_ir::{TypedBinding, TypedExpr, TypedExprKind, TypedForHeader};
use nia_control_ir::{
    ControlBlockId, ControlBody, ControlDeferBody, ControlOp, ControlScopeId, ControlTerminator,
};
use nia_diagnostic::Diagnostic;
use nia_llvm::{basic_block::BasicBlock, values::IntValue};
use nia_span::Span;

use super::FunctionCodegen;

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(crate) fn emit_control_body(&mut self, body: &ControlBody) -> Result<(), Diagnostic> {
        let physical_entry = self
            .module
            .context
            .append_basic_block(self.llvm_function, "entry")?;
        let mut llvm_blocks = std::collections::HashMap::new();
        for block in &body.blocks {
            let name = if block.id == body.entry {
                "cir.entry".to_string()
            } else {
                format!("cir.bb{}", block.id.0)
            };
            let llvm_block = self
                .module
                .context
                .append_basic_block(self.llvm_function, &name)?;
            llvm_blocks.insert(block.id, llvm_block);
        }

        let Some(entry) = llvm_blocks.get(&body.entry).copied() else {
            return Err(self.error(body.span, "control body has no entry block"));
        };
        self.builder.position_at_end(physical_entry);
        self.out_ptr = self.function_out_ptr()?;
        self.alloc_control_locals(body)?;
        self.store_params()?;
        for scope in &body.scopes {
            if scope.parent.is_none() {
                self.ensure_control_defer_scope(scope.id);
            }
        }
        self.builder
            .build_unconditional_branch(entry)
            .map_err(|_| self.error(body.span, "failed to branch to control entry"))?;

        for block in &body.blocks {
            let Some(llvm_block) = llvm_blocks.get(&block.id).copied() else {
                return Err(self.error(block.span, "missing control block"));
            };
            self.builder.position_at_end(llvm_block);
            self.ensure_control_defer_scope(block.scope);
            self.active_control_scope = Some(block.scope);
            if self.current_block_has_terminator() {
                continue;
            }
            for op in &block.ops {
                self.emit_control_op(block.span, op)?;
                if self.current_block_has_terminator() {
                    break;
                }
            }
            if !self.current_block_has_terminator() {
                self.emit_control_terminator(body, block.id, &block.terminator, &llvm_blocks)?;
            }
        }
        self.active_control_scope = None;

        Ok(())
    }

    pub(super) fn emit_defer_control_body(
        &mut self,
        body: &ControlDeferBody,
    ) -> Result<(), Diagnostic> {
        let mut llvm_blocks = std::collections::HashMap::new();
        for block in &body.blocks {
            let name = if block.id == body.entry {
                "defer.entry".to_string()
            } else {
                format!("defer.bb{}", block.id.0)
            };
            let llvm_block = self
                .module
                .context
                .append_basic_block(self.llvm_function, &name)?;
            llvm_blocks.insert(block.id, llvm_block);
        }
        let defer_end = self
            .module
            .context
            .append_basic_block(self.llvm_function, "defer.end")?;
        let Some(entry) = llvm_blocks.get(&body.entry).copied() else {
            return Err(self.error(body.span, "defer control body has no entry block"));
        };
        let saved_defer_scope_len = self.defer_scopes.len();
        let saved_control_defer_scopes = self.control_defer_scopes.clone();
        self.control_defer_scopes = std::collections::HashMap::new();
        self.builder
            .build_unconditional_branch(entry)
            .map_err(|_| self.error(body.span, "failed to branch to defer control entry"))?;
        let saved_active_scope = self.active_control_scope;

        for scope in &body.scopes {
            if scope.parent.is_none() {
                self.ensure_control_defer_scope(scope.id);
            }
        }

        for block in &body.blocks {
            let Some(llvm_block) = llvm_blocks.get(&block.id).copied() else {
                return Err(self.error(block.span, "missing defer control block"));
            };
            self.builder.position_at_end(llvm_block);
            self.ensure_control_defer_scope(block.scope);
            self.active_control_scope = Some(block.scope);
            if self.current_block_has_terminator() {
                continue;
            }
            for op in &block.ops {
                self.emit_control_op(block.span, op)?;
                if self.current_block_has_terminator() {
                    break;
                }
            }
            if !self.current_block_has_terminator() {
                self.emit_defer_control_terminator(
                    body,
                    block.id,
                    &block.terminator,
                    &llvm_blocks,
                    defer_end,
                )?;
            }
        }
        self.active_control_scope = saved_active_scope;
        self.defer_scopes.truncate(saved_defer_scope_len);
        self.control_defer_scopes = saved_control_defer_scopes;
        self.builder.position_at_end(defer_end);
        Ok(())
    }

    fn ensure_control_defer_scope(&mut self, scope: ControlScopeId) {
        if !self.control_defer_scopes.contains_key(&scope) {
            let index = self.push_control_defer_scope(scope);
            self.control_defer_scopes.insert(scope, index);
        }
    }

    fn emit_control_op(&mut self, span: Span, op: &ControlOp) -> Result<(), Diagnostic> {
        match op {
            ControlOp::Binding(binding) => self.emit_binding(span, binding),
            ControlOp::StoreLocal {
                local_id,
                value,
                span,
            } => self.emit_store_local(*span, *local_id, value),
            ControlOp::Expr(expr) => self.emit_effect_expr(expr),
            ControlOp::Defer(expr) => self.register_defer(span, expr),
        }
    }

    fn emit_control_terminator(
        &mut self,
        body: &ControlBody,
        block: ControlBlockId,
        terminator: &ControlTerminator,
        llvm_blocks: &std::collections::HashMap<ControlBlockId, BasicBlock<'ctx>>,
    ) -> Result<(), Diagnostic> {
        match terminator {
            ControlTerminator::Next { target, span }
            | ControlTerminator::Branch { target, span } => {
                self.emit_control_edge_defers(body, block, *target, *span)?;
                let target = self.llvm_control_block(*span, *target, llvm_blocks)?;
                self.builder
                    .build_unconditional_branch(target)
                    .map_err(|_| self.error(*span, "failed to build control branch"))?;
            }
            ControlTerminator::Return { value, span } => {
                self.emit_control_return(body, block, *span, value.as_ref())?;
            }
            ControlTerminator::Tail { value, span } => {
                self.emit_control_tail(body, block, *span, value.as_ref())?;
            }
            ControlTerminator::If {
                cond,
                then_target,
                else_target,
                span,
            } => {
                let cond = self.emit_expr(cond)?.into_int_value()?;
                let then_block = self.llvm_control_block(*span, *then_target, llvm_blocks)?;
                let else_block = self.llvm_control_block(*span, *else_target, llvm_blocks)?;
                self.builder
                    .build_conditional_branch(cond, then_block, else_block)
                    .map_err(|_| self.error(*span, "failed to build control if branch"))?;
            }
            ControlTerminator::Switch {
                target,
                arms,
                default,
                fallback,
                span,
            } => {
                let target = self.emit_expr(target)?.into_int_value()?;
                let default = default.unwrap_or(*fallback);
                let default_block = self.llvm_control_block(*span, default, llvm_blocks)?;
                let mut cases = Vec::new();
                for arm in arms {
                    let value = self.emit_switch_pattern_value(&arm.pattern)?;
                    let block = self.llvm_control_block(*span, arm.target, llvm_blocks)?;
                    cases.push((value, block));
                }
                self.builder
                    .build_switch(target, default_block, &cases)
                    .map_err(|_| self.error(*span, "failed to build control switch"))?;
            }
            ControlTerminator::Loop {
                header,
                body: loop_body,
                break_target,
                span,
                ..
            } => {
                let body_block = self.llvm_control_block(*span, *loop_body, llvm_blocks)?;
                let break_block = self.llvm_control_block(*span, *break_target, llvm_blocks)?;
                self.emit_control_loop_header(*span, header, body_block, break_block)?;
            }
        }
        Ok(())
    }

    fn emit_defer_control_terminator(
        &mut self,
        body: &ControlDeferBody,
        block: ControlBlockId,
        terminator: &ControlTerminator,
        llvm_blocks: &std::collections::HashMap<ControlBlockId, BasicBlock<'ctx>>,
        defer_end: BasicBlock<'ctx>,
    ) -> Result<(), Diagnostic> {
        match terminator {
            ControlTerminator::Next { target, span }
            | ControlTerminator::Branch { target, span } => {
                self.emit_defer_control_edge_defers(body, block, *target, *span)?;
                let target = self.llvm_control_block(*span, *target, llvm_blocks)?;
                self.builder
                    .build_unconditional_branch(target)
                    .map_err(|_| self.error(*span, "failed to build defer control branch"))?;
            }
            ControlTerminator::Return { span, .. } => {
                return Err(self.error(*span, "`return` is not valid in defer control IR"));
            }
            ControlTerminator::Tail { span, .. } => {
                self.emit_defer_control_tail_defers(body, block, *span)?;
                self.builder
                    .build_unconditional_branch(defer_end)
                    .map_err(|_| self.error(*span, "failed to leave defer control body"))?;
            }
            ControlTerminator::If {
                cond,
                then_target,
                else_target,
                span,
            } => {
                let cond = self.emit_expr(cond)?.into_int_value()?;
                let then_block = self.llvm_control_block(*span, *then_target, llvm_blocks)?;
                let else_block = self.llvm_control_block(*span, *else_target, llvm_blocks)?;
                self.builder
                    .build_conditional_branch(cond, then_block, else_block)
                    .map_err(|_| self.error(*span, "failed to build defer control if branch"))?;
            }
            ControlTerminator::Switch {
                target,
                arms,
                default,
                fallback,
                span,
            } => {
                let target = self.emit_expr(target)?.into_int_value()?;
                let default = default.unwrap_or(*fallback);
                let default_block = self.llvm_control_block(*span, default, llvm_blocks)?;
                let mut cases = Vec::new();
                for arm in arms {
                    let value = self.emit_switch_pattern_value(&arm.pattern)?;
                    let block = self.llvm_control_block(*span, arm.target, llvm_blocks)?;
                    cases.push((value, block));
                }
                self.builder
                    .build_switch(target, default_block, &cases)
                    .map_err(|_| self.error(*span, "failed to build defer control switch"))?;
            }
            ControlTerminator::Loop {
                header,
                body: loop_body,
                break_target,
                span,
                ..
            } => {
                let body_block = self.llvm_control_block(*span, *loop_body, llvm_blocks)?;
                let break_block = self.llvm_control_block(*span, *break_target, llvm_blocks)?;
                self.emit_control_loop_header(*span, header, body_block, break_block)?;
            }
        }
        Ok(())
    }

    fn emit_control_loop_header(
        &mut self,
        span: Span,
        header: &TypedForHeader,
        body_block: BasicBlock<'ctx>,
        break_block: BasicBlock<'ctx>,
    ) -> Result<(), Diagnostic> {
        match header {
            TypedForHeader::Infinite | TypedForHeader::CStyle { cond: None, .. } => {
                self.builder
                    .build_unconditional_branch(body_block)
                    .map_err(|_| self.error(span, "failed to build control loop branch"))?;
            }
            TypedForHeader::Condition(cond) => {
                let cond = self.emit_expr(cond)?.into_int_value()?;
                self.builder
                    .build_conditional_branch(cond, body_block, break_block)
                    .map_err(|_| self.error(span, "failed to build control loop branch"))?;
            }
            TypedForHeader::CStyle {
                cond: Some(cond), ..
            } => {
                let cond = self.emit_expr(cond)?.into_int_value()?;
                self.builder
                    .build_conditional_branch(cond, body_block, break_block)
                    .map_err(|_| self.error(span, "failed to build control loop branch"))?;
            }
        }
        Ok(())
    }

    fn emit_control_edge_defers(
        &mut self,
        body: &ControlBody,
        from: ControlBlockId,
        to: ControlBlockId,
        span: Span,
    ) -> Result<(), Diagnostic> {
        let Some(scopes) = body.edge_exited_scopes(from, to) else {
            return Err(self.error(span, "invalid control branch scopes"));
        };
        for scope in scopes {
            self.emit_control_scope_defers(span, scope)?;
        }
        Ok(())
    }

    fn emit_control_return(
        &mut self,
        body: &ControlBody,
        block: ControlBlockId,
        span: Span,
        value: Option<&TypedExpr>,
    ) -> Result<(), Diagnostic> {
        if let Some(value) = value {
            let value = self.emit_expr(value)?;
            self.emit_control_tail_defers(body, block, span)?;
            if self.is_never(self.function.return_type) {
                self.builder
                    .build_unreachable()
                    .map_err(|_| self.error(span, "failed to build never return"))?;
            } else {
                self.emit_return_value(span, value)?;
            }
        } else {
            self.emit_control_tail_defers(body, block, span)?;
            if self.is_never(self.function.return_type) {
                self.builder
                    .build_unreachable()
                    .map_err(|_| self.error(span, "failed to build never return"))?;
            } else {
                self.builder
                    .build_return(None)
                    .map_err(|_| self.error(span, "failed to build void return"))?;
            }
        }
        Ok(())
    }

    fn emit_control_tail(
        &mut self,
        body: &ControlBody,
        block: ControlBlockId,
        span: Span,
        value: Option<&TypedExpr>,
    ) -> Result<(), Diagnostic> {
        let Some(value) = value else {
            if self.is_void(self.function.return_type) {
                self.emit_control_tail_defers(body, block, span)?;
                self.builder
                    .build_return(None)
                    .map_err(|_| self.error(span, "failed to build void return"))?;
            } else if self.is_never(self.function.return_type) {
                self.emit_control_tail_defers(body, block, span)?;
                self.builder
                    .build_unreachable()
                    .map_err(|_| self.error(span, "failed to build never function unreachable"))?;
            }
            return Ok(());
        };
        if self.is_zero_sized(value.ty) {
            self.emit_effect_expr(value)?;
            self.emit_control_tail_defers(body, block, span)?;
            self.builder
                .build_return(None)
                .map_err(|_| self.error(span, "failed to build void return"))?;
            return Ok(());
        }
        let value = self.emit_expr(value)?;
        if self.current_block_has_terminator() {
            return Ok(());
        }
        self.emit_control_tail_defers(body, block, span)?;
        self.emit_return_value(span, value)
    }

    fn emit_control_tail_defers(
        &mut self,
        body: &ControlBody,
        block: ControlBlockId,
        span: Span,
    ) -> Result<(), Diagnostic> {
        let Some(scopes) = body.return_exited_scopes(block) else {
            return Err(self.error(span, "invalid control tail scopes"));
        };
        for scope in scopes {
            self.emit_control_scope_defers(span, scope)?;
        }
        Ok(())
    }

    fn emit_defer_control_edge_defers(
        &mut self,
        body: &ControlDeferBody,
        from: ControlBlockId,
        to: ControlBlockId,
        span: Span,
    ) -> Result<(), Diagnostic> {
        let Some(scopes) = body.edge_exited_scopes(from, to) else {
            return Err(self.error(span, "invalid defer control branch scopes"));
        };
        for scope in scopes {
            self.emit_control_scope_defers(span, scope)?;
        }
        Ok(())
    }

    fn emit_defer_control_tail_defers(
        &mut self,
        body: &ControlDeferBody,
        block: ControlBlockId,
        span: Span,
    ) -> Result<(), Diagnostic> {
        let Some(scopes) = body.return_exited_scopes(block) else {
            return Err(self.error(span, "invalid defer control tail scopes"));
        };
        for scope in scopes {
            self.emit_control_scope_defers(span, scope)?;
        }
        Ok(())
    }

    fn llvm_control_block(
        &self,
        span: Span,
        id: ControlBlockId,
        llvm_blocks: &std::collections::HashMap<ControlBlockId, BasicBlock<'ctx>>,
    ) -> Result<BasicBlock<'ctx>, Diagnostic> {
        llvm_blocks
            .get(&id)
            .copied()
            .ok_or_else(|| self.error(span, "missing control branch target"))
    }

    pub(super) fn emit_switch_pattern_value(
        &mut self,
        pattern: &TypedExpr,
    ) -> Result<IntValue<'ctx>, Diagnostic> {
        Ok(self.emit_expr(pattern)?.into_int_value()?)
    }

    pub(super) fn emit_binding(
        &mut self,
        span: Span,
        binding: &TypedBinding,
    ) -> Result<(), Diagnostic> {
        if let Some(value) = &binding.value {
            if self.is_zero_sized(binding.ty) {
                self.emit_effect_expr(value)?;
                return Ok(());
            }
            let value = self.emit_expr(value)?;
            let Some(ptr) = self.locals.get(&binding.local_id).copied() else {
                return Err(self.error(span, "missing local binding storage"));
            };
            self.builder
                .build_store(ptr, value)
                .map_err(|_| self.error(span, "failed to store local binding"))?;
        }
        Ok(())
    }

    fn emit_store_local(
        &mut self,
        span: Span,
        local_id: nia_ids::LocalId,
        value: &TypedExpr,
    ) -> Result<(), Diagnostic> {
        if self.is_zero_sized(value.ty) {
            self.emit_effect_expr(value)?;
            return Ok(());
        }
        let value = self.emit_expr(value)?;
        let Some(ptr) = self.locals.get(&local_id).copied() else {
            return Err(self.error(span, "missing local binding storage"));
        };
        self.builder
            .build_store(ptr, value)
            .map_err(|_| self.error(span, "failed to store local binding"))?;
        Ok(())
    }

    pub(super) fn emit_effect_expr(&mut self, expr: &TypedExpr) -> Result<(), Diagnostic> {
        match &expr.kind {
            TypedExprKind::Assign { place, op, rhs } => {
                let value = self.emit_expr(rhs)?;
                self.emit_assign(expr.span, place, *op, value)
            }
            TypedExprKind::Discard(inner) => self.emit_effect_expr(inner),
            TypedExprKind::Call { callee, args } => {
                let _ = self.emit_call_raw(expr, callee, args)?;
                Ok(())
            }
            TypedExprKind::InlineAsm(asm) => self.emit_inline_asm(asm),
            TypedExprKind::StructLiteral { .. }
            | TypedExprKind::Local(_)
            | TypedExprKind::Global(_)
            | TypedExprKind::CStringPointer { .. } => Ok(()),
            TypedExprKind::Block(_) | TypedExprKind::If { .. } | TypedExprKind::Switch(_) => {
                Err(self.error(
                    expr.span,
                    "control expression was not lowered to control IR",
                ))
            }
            _ => {
                let _ = self.emit_expr(expr)?;
                Ok(())
            }
        }
    }
}
