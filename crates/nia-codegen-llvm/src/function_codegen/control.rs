// SPDX-License-Identifier: GPL-3.0-or-later
use nia_backend_ir::{
    TypedBinding, TypedBody, TypedExpr, TypedExprKind, TypedFor, TypedForHeader, TypedForInit,
    TypedStmt, TypedStmtKind, TypedSwitch, TypedSwitchArmBody, TypedSwitchPattern,
};
use nia_diagnostic::Diagnostic;
use nia_llvm::values::{BasicValueEnum, IntValue};
use nia_span::Span;

use super::{FunctionCodegen, LoopTargets};

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(super) fn emit_stmt(&mut self, stmt: &TypedStmt) -> Result<(), Diagnostic> {
        match &stmt.kind {
            TypedStmtKind::Binding(binding) => self.emit_binding(stmt.span, binding),
            TypedStmtKind::Expr(expr) => {
                if self.is_void_expr(expr) {
                    self.emit_void_expr(expr)?;
                } else {
                    let _ = self.emit_expr(expr)?;
                }
                Ok(())
            }
            TypedStmtKind::Defer(expr) => self.register_defer(stmt.span, expr),
            TypedStmtKind::Return(value) => {
                if let Some(value) = value {
                    let value = self.emit_expr(value)?;
                    self.emit_all_defers(stmt.span)?;
                    if self.is_never(self.function.return_type) {
                        self.builder
                            .build_unreachable()
                            .map_err(|_| self.error(stmt.span, "failed to build never return"))?;
                    } else {
                        self.builder
                            .build_return(Some(&value))
                            .map_err(|_| self.error(stmt.span, "failed to build return"))?;
                    }
                } else {
                    self.emit_all_defers(stmt.span)?;
                    if self.is_never(self.function.return_type) {
                        self.builder
                            .build_unreachable()
                            .map_err(|_| self.error(stmt.span, "failed to build never return"))?;
                    } else {
                        self.builder
                            .build_return(None)
                            .map_err(|_| self.error(stmt.span, "failed to build void return"))?;
                    }
                }
                Ok(())
            }
            TypedStmtKind::Break => self.emit_break(stmt.span),
            TypedStmtKind::Continue => self.emit_continue(stmt.span),
            TypedStmtKind::For(for_stmt) => self.emit_for_stmt(stmt.span, for_stmt),
            TypedStmtKind::Switch(switch) => self.emit_switch_stmt(stmt.span, switch),
        }
    }

    fn emit_binding(&mut self, span: Span, binding: &TypedBinding) -> Result<(), Diagnostic> {
        if let Some(value) = &binding.value {
            if self.is_void_expr(value) {
                self.emit_void_expr(value)?;
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

    fn emit_break(&mut self, span: Span) -> Result<(), Diagnostic> {
        let Some(targets) = self.loops.last().copied() else {
            return Err(self.error(span, "`break` is not inside a loop"));
        };
        self.emit_loop_exit_defers(span)?;
        self.builder
            .build_unconditional_branch(targets.break_block)
            .map_err(|_| self.error(span, "failed to build break branch"))?;
        Ok(())
    }

    fn emit_continue(&mut self, span: Span) -> Result<(), Diagnostic> {
        let Some(targets) = self.loops.last().copied() else {
            return Err(self.error(span, "`continue` is not inside a loop"));
        };
        self.emit_loop_exit_defers(span)?;
        self.builder
            .build_unconditional_branch(targets.continue_block)
            .map_err(|_| self.error(span, "failed to build continue branch"))?;
        Ok(())
    }

    fn emit_for_stmt(&mut self, span: Span, for_stmt: &TypedFor) -> Result<(), Diagnostic> {
        if let TypedForHeader::CStyle {
            init: Some(init), ..
        } = &for_stmt.header
        {
            self.emit_for_init(span, init)?;
        }

        let cond_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "for.cond");
        let body_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "for.body");
        let step_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "for.step");
        let end_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "for.end");

        if !self.current_block_has_terminator() {
            self.builder
                .build_unconditional_branch(cond_block)
                .map_err(|_| self.error(span, "failed to branch to loop condition"))?;
        }

        self.builder.position_at_end(cond_block);
        match &for_stmt.header {
            TypedForHeader::Infinite | TypedForHeader::CStyle { cond: None, .. } => {
                self.builder
                    .build_unconditional_branch(body_block)
                    .map_err(|_| self.error(span, "failed to build loop condition branch"))?;
            }
            TypedForHeader::Condition(cond) => {
                let cond = self.emit_expr(cond)?.into_int_value();
                self.builder
                    .build_conditional_branch(cond, body_block, end_block)
                    .map_err(|_| self.error(span, "failed to build loop condition branch"))?;
            }
            TypedForHeader::CStyle {
                cond: Some(cond), ..
            } => {
                let cond = self.emit_expr(cond)?.into_int_value();
                self.builder
                    .build_conditional_branch(cond, body_block, end_block)
                    .map_err(|_| self.error(span, "failed to build loop condition branch"))?;
            }
        }

        let continue_block = match &for_stmt.header {
            TypedForHeader::CStyle { step: Some(_), .. } => step_block,
            _ => cond_block,
        };
        self.loops.push(LoopTargets {
            break_block: end_block,
            continue_block,
        });

        self.builder.position_at_end(body_block);
        self.emit_body_contents(&for_stmt.body)?;
        if !self.current_block_has_terminator() {
            self.builder
                .build_unconditional_branch(step_block)
                .map_err(|_| self.error(span, "failed to branch to loop step"))?;
        }
        self.loops.pop();

        self.builder.position_at_end(step_block);
        if let TypedForHeader::CStyle {
            step: Some(step), ..
        } = &for_stmt.header
        {
            self.emit_void_expr(step)?;
        }
        if !self.current_block_has_terminator() {
            self.builder
                .build_unconditional_branch(cond_block)
                .map_err(|_| self.error(span, "failed to branch to loop condition"))?;
        }

        self.builder.position_at_end(end_block);
        Ok(())
    }

    fn emit_for_init(&mut self, span: Span, init: &TypedForInit) -> Result<(), Diagnostic> {
        match init {
            TypedForInit::Binding(binding) => self.emit_binding(span, binding),
            TypedForInit::Expr(expr) => self.emit_void_expr(expr),
        }
    }

    fn emit_switch_stmt(&mut self, span: Span, switch: &TypedSwitch) -> Result<(), Diagnostic> {
        let target = self.emit_expr(&switch.target)?.into_int_value();
        let end_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "switch.end");
        let default_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "switch.default");
        let mut arm_blocks = Vec::new();
        let mut cases = Vec::new();
        let mut default_arm = None;

        for (index, arm) in switch.arms.iter().enumerate() {
            match &arm.pattern {
                TypedSwitchPattern::Default => default_arm = Some(&arm.body),
                TypedSwitchPattern::Expr(pattern) => {
                    let block = self
                        .module
                        .context
                        .append_basic_block(self.llvm_function, &format!("switch.arm.{index}"));
                    let value = self.emit_switch_pattern_value(pattern)?;
                    cases.push((value, block));
                    arm_blocks.push((block, &arm.body));
                }
            }
        }

        self.builder
            .build_switch(target, default_block, &cases)
            .map_err(|_| self.error(span, "failed to build switch"))?;

        for (block, body) in arm_blocks {
            self.builder.position_at_end(block);
            self.emit_switch_arm_body(body)?;
            if !self.current_block_has_terminator() {
                self.builder
                    .build_unconditional_branch(end_block)
                    .map_err(|_| self.error(span, "failed to branch from switch arm"))?;
            }
        }

        self.builder.position_at_end(default_block);
        if let Some(body) = default_arm {
            self.emit_switch_arm_body(body)?;
        }
        if !self.current_block_has_terminator() {
            self.builder
                .build_unconditional_branch(end_block)
                .map_err(|_| self.error(span, "failed to branch from switch default"))?;
        }

        self.builder.position_at_end(end_block);
        Ok(())
    }

    fn emit_switch_pattern_value(
        &mut self,
        pattern: &TypedExpr,
    ) -> Result<IntValue<'ctx>, Diagnostic> {
        Ok(self.emit_expr(pattern)?.into_int_value())
    }

    fn emit_switch_arm_body(&mut self, body: &TypedSwitchArmBody) -> Result<(), Diagnostic> {
        match body {
            TypedSwitchArmBody::Expr(expr) => self.emit_void_expr(expr),
            TypedSwitchArmBody::Stmt(stmt) => self.emit_stmt(stmt),
            TypedSwitchArmBody::Block(body) => self.emit_body_contents(body),
        }
    }

    pub(super) fn emit_body_contents(&mut self, body: &TypedBody) -> Result<(), Diagnostic> {
        let scope = self.push_defer_scope();
        for stmt in &body.stmts {
            self.emit_stmt(stmt)?;
            if self.current_block_has_terminator() {
                break;
            }
        }
        if !self.current_block_has_terminator()
            && let Some(tail) = &body.tail
        {
            if self.is_void_expr(tail) {
                self.emit_void_expr(tail)?;
            } else {
                let _ = self.emit_expr(tail)?;
            }
        }
        self.pop_defer_scope_to(scope, !self.current_block_has_terminator())?;
        Ok(())
    }

    pub(super) fn emit_void_expr(&mut self, expr: &TypedExpr) -> Result<(), Diagnostic> {
        match &expr.kind {
            TypedExprKind::Assign { place, op, rhs } => {
                let value = self.emit_expr(rhs)?;
                self.emit_assign(expr.span, place, *op, value)
            }
            TypedExprKind::Call { callee, args } => {
                let _ = self.emit_call_raw(expr, callee, args)?;
                Ok(())
            }
            TypedExprKind::InlineAsm(asm) => self.emit_inline_asm(asm),
            TypedExprKind::Block(body) => self.emit_zero_sized_body(body),
            TypedExprKind::StructLiteral { .. } => Ok(()),
            TypedExprKind::Local(_) | TypedExprKind::Global(_) => Ok(()),
            TypedExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.emit_void_if_expr(expr.span, cond, then_branch, else_branch.as_deref()),
            _ => {
                let _ = self.emit_expr(expr)?;
                Ok(())
            }
        }
    }

    fn is_void_expr(&self, expr: &TypedExpr) -> bool {
        self.is_void(expr.ty)
            || self.is_never(expr.ty)
            || self
                .module
                .layout_of(expr.ty)
                .is_some_and(|layout| layout.size == 0)
            || matches!(expr.kind, TypedExprKind::Assign { .. })
    }

    pub(super) fn emit_if_expr(
        &mut self,
        span: Span,
        cond: &TypedExpr,
        then_branch: &TypedBody,
        else_branch: Option<&TypedExpr>,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(else_branch) = else_branch else {
            return Err(self.error(span, "value if-expression requires an else branch"));
        };
        let cond = self.emit_expr(cond)?.into_int_value();
        let then_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "if.then");
        let else_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "if.else");
        let merge_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "if.end");
        self.builder
            .build_conditional_branch(cond, then_block, else_block)
            .map_err(|_| self.error(span, "failed to build if branch"))?;

        self.builder.position_at_end(then_block);
        let then_value = self.emit_block_expr(then_branch)?;
        let then_end = self.builder.get_insert_block();
        if !self.current_block_has_terminator() {
            self.builder
                .build_unconditional_branch(merge_block)
                .map_err(|_| self.error(span, "failed to build then merge branch"))?;
        }

        self.builder.position_at_end(else_block);
        let else_value = self.emit_expr(else_branch)?;
        let else_end = self.builder.get_insert_block();
        if !self.current_block_has_terminator() {
            self.builder
                .build_unconditional_branch(merge_block)
                .map_err(|_| self.error(span, "failed to build else merge branch"))?;
        }

        self.builder.position_at_end(merge_block);
        let phi = self
            .builder
            .build_phi(then_value.get_type(), "iftmp")
            .map_err(|_| self.error(span, "failed to build if phi"))?;
        let Some(then_end) = then_end else {
            return Err(self.error(span, "missing then block for if phi"));
        };
        let Some(else_end) = else_end else {
            return Err(self.error(span, "missing else block for if phi"));
        };
        phi.add_incoming(&[(&then_value, then_end), (&else_value, else_end)]);
        Ok(phi.as_basic_value())
    }

    pub(super) fn emit_void_if_expr(
        &mut self,
        span: Span,
        cond: &TypedExpr,
        then_branch: &TypedBody,
        else_branch: Option<&TypedExpr>,
    ) -> Result<(), Diagnostic> {
        let cond = self.emit_expr(cond)?.into_int_value();
        let then_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "if.then");
        let else_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "if.else");
        let merge_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "if.end");
        self.builder
            .build_conditional_branch(cond, then_block, else_block)
            .map_err(|_| self.error(span, "failed to build if branch"))?;

        self.builder.position_at_end(then_block);
        self.emit_body_contents(then_branch)?;
        if !self.current_block_has_terminator() {
            self.builder
                .build_unconditional_branch(merge_block)
                .map_err(|_| self.error(span, "failed to build then merge branch"))?;
        }

        self.builder.position_at_end(else_block);
        if let Some(else_branch) = else_branch {
            self.emit_void_expr(else_branch)?;
        }
        if !self.current_block_has_terminator() {
            self.builder
                .build_unconditional_branch(merge_block)
                .map_err(|_| self.error(span, "failed to build else merge branch"))?;
        }

        self.builder.position_at_end(merge_block);
        Ok(())
    }
}
