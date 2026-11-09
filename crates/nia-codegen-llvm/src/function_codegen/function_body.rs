// SPDX-License-Identifier: GPL-3.0-or-later
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    FunctionBinding, FunctionBlockId, FunctionBody, FunctionDeferBody, FunctionErrorUnionTag,
    FunctionExpr, FunctionExprKind, FunctionForHeader, FunctionIrError, FunctionOp,
    FunctionOptionalTag, FunctionScopeId, FunctionTerminator, FunctionTryKind,
    validate_function_body,
};
use nia_llvm::{
    IntPredicate,
    basic_block::BasicBlock,
    values::{BasicValueEnum, IntValue, PointerValue},
};
use nia_span::Span;

use super::FunctionCodegen;

struct TryTerminatorInput<'b, 'ctx> {
    body: &'b FunctionBody,
    block: FunctionBlockId,
    span: Span,
    value: &'b FunctionExpr,
    kind: FunctionTryKind,
    error_conversion: Option<&'b FunctionExpr>,
    success_local: nia_ids::LocalId,
    success_target: FunctionBlockId,
    llvm_blocks: &'b std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
}

/// The failed operand of a `.?` propagation and how its failure is returned.
#[derive(Clone, Copy)]
struct TryFailure<'b, 'ctx> {
    span: Span,
    aggregate: PointerValue<'ctx>,
    aggregate_ty: nia_ids::InternedTyId,
    kind: FunctionTryKind,
    error_conversion: Option<&'b FunctionExpr>,
}

struct TryFailureReturn<'b, 'ctx> {
    body: &'b FunctionBody,
    block: FunctionBlockId,
    failure: TryFailure<'b, 'ctx>,
    outer_blocks: &'b std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
}

struct DeferTryTerminatorInput<'b, 'ctx> {
    body: &'b FunctionDeferBody,
    block: FunctionBlockId,
    span: Span,
    value: &'b FunctionExpr,
    kind: FunctionTryKind,
    error_conversion: Option<&'b FunctionExpr>,
    success_local: nia_ids::LocalId,
    success_target: FunctionBlockId,
    llvm_blocks: &'b std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
    outer_blocks: &'b std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
}

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(crate) fn emit_function_body(&mut self, body: &FunctionBody) -> Result<(), Diagnostic> {
        validate_function_body(body).map_err(function_ir_diagnostic)?;
        let physical_entry = self
            .module
            .context
            .append_basic_block(self.llvm_function, "entry")?;
        let mut llvm_blocks = std::collections::HashMap::new();
        for block in &body.blocks {
            let name = if block.id == body.entry {
                "fir.entry".to_string()
            } else {
                format!("fir.bb{}", block.id.0)
            };
            let llvm_block = self
                .module
                .context
                .append_basic_block(self.llvm_function, &name)?;
            llvm_blocks.insert(block.id, llvm_block);
        }

        let Some(entry) = llvm_blocks.get(&body.entry).copied() else {
            return Err(self.error(body.span, "function body has no entry block"));
        };
        self.builder.position_at_end(physical_entry);
        self.out_ptr = self.function_out_ptr()?;
        self.caller_location = self.function_caller_location()?;
        self.alloc_function_return_storage(body)?;
        self.alloc_function_locals(body)?;
        self.store_params()?;
        for scope in &body.scopes {
            if scope.parent.is_none() {
                self.ensure_function_defer_scope(scope.id);
            }
        }
        self.builder
            .build_unconditional_branch(entry)
            .map_err(|_| self.error(body.span, "failed to branch to function entry"))?;

        for block in &body.blocks {
            let Some(llvm_block) = llvm_blocks.get(&block.id).copied() else {
                return Err(self.error(block.span, "missing function block"));
            };
            self.builder.position_at_end(llvm_block);
            self.ensure_function_defer_scope(block.scope);
            self.active_function_scope = Some(block.scope);
            if self.current_block_has_terminator() {
                continue;
            }
            for op in &block.ops {
                self.emit_function_op(block.span, op)?;
                if self.current_block_has_terminator() {
                    break;
                }
            }
            if !self.current_block_has_terminator() {
                self.emit_function_terminator(body, block.id, &block.terminator, &llvm_blocks)?;
            }
        }
        self.active_function_scope = None;
        self.emit_pending_function_return_cleanups(&llvm_blocks)?;

        Ok(())
    }

    /// Storage that keeps an immediate result while deferred code runs. A
    /// memory result is already in the result slot.
    fn alloc_function_return_storage(&mut self, body: &FunctionBody) -> Result<(), Diagnostic> {
        let has_defer = body
            .blocks
            .iter()
            .flat_map(|block| &block.ops)
            .any(|op| matches!(op, FunctionOp::Defer(_)));
        if !has_defer
            || !matches!(
                self.module
                    .classify_function_return(self.function.return_type),
                crate::module_codegen::AbiReturn::Direct(_)
            )
        {
            return Ok(());
        }
        self.function_return_storage =
            Some(self.temporary(self.function.return_type, "return.cleanup", body.span)?);
        Ok(())
    }

    pub(super) fn emit_defer_function_body(
        &mut self,
        body: &FunctionDeferBody,
        outer_blocks: &std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
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
        let mut defer_outer_blocks = outer_blocks.clone();
        defer_outer_blocks.extend(llvm_blocks.iter().map(|(id, block)| (*id, *block)));
        let defer_end = self
            .module
            .context
            .append_basic_block(self.llvm_function, "defer.end")?;
        let Some(entry) = llvm_blocks.get(&body.entry).copied() else {
            return Err(self.error(body.span, "defer function body has no entry block"));
        };
        let saved_defer_scope_len = self.defer_scopes.len();
        let saved_function_defer_scopes = self.function_defer_scopes.clone();
        self.function_defer_scopes = std::collections::HashMap::new();
        self.builder
            .build_unconditional_branch(entry)
            .map_err(|_| self.error(body.span, "failed to branch to defer function entry"))?;
        let saved_active_scope = self.active_function_scope;

        for scope in &body.scopes {
            if scope.parent.is_none() {
                self.ensure_function_defer_scope(scope.id);
            }
        }

        for block in &body.blocks {
            let Some(llvm_block) = llvm_blocks.get(&block.id).copied() else {
                return Err(self.error(block.span, "missing defer function block"));
            };
            self.builder.position_at_end(llvm_block);
            self.ensure_function_defer_scope(block.scope);
            self.active_function_scope = Some(block.scope);
            if self.current_block_has_terminator() {
                continue;
            }
            for op in &block.ops {
                self.emit_function_op(block.span, op)?;
                if self.current_block_has_terminator() {
                    break;
                }
            }
            if !self.current_block_has_terminator() {
                self.emit_defer_function_terminator(
                    body,
                    block.id,
                    &block.terminator,
                    &llvm_blocks,
                    &defer_outer_blocks,
                    defer_end,
                )?;
            }
        }
        self.active_function_scope = saved_active_scope;
        self.defer_scopes.truncate(saved_defer_scope_len);
        self.function_defer_scopes = saved_function_defer_scopes;
        self.builder.position_at_end(defer_end);
        Ok(())
    }

    fn ensure_function_defer_scope(&mut self, scope: FunctionScopeId) {
        if !self.function_defer_scopes.contains_key(&scope) {
            let index = self.push_function_defer_scope(scope);
            self.function_defer_scopes.insert(scope, index);
        }
    }

    fn emit_function_op(&mut self, span: Span, op: &FunctionOp) -> Result<(), Diagnostic> {
        match op {
            FunctionOp::Binding(binding) => self.emit_binding(span, binding),
            FunctionOp::StoreLocal {
                local_id,
                value,
                span,
            } => self.emit_store_local(*span, *local_id, value),
            FunctionOp::MemoryIntrinsic(memory) => self.emit_memory_intrinsic(memory),
            FunctionOp::Expr(expr) => self.emit_effect_expr(expr),
            FunctionOp::Defer(expr) => self.register_defer(span, expr),
        }
    }

    fn emit_function_terminator(
        &mut self,
        body: &FunctionBody,
        block: FunctionBlockId,
        terminator: &FunctionTerminator,
        llvm_blocks: &std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
    ) -> Result<(), Diagnostic> {
        match terminator {
            FunctionTerminator::Error { span } => {
                self.builder
                    .build_unreachable()
                    .map_err(|_| self.error(*span, "failed to build unreachable"))?;
            }
            FunctionTerminator::Next { target, span }
            | FunctionTerminator::Branch { target, span } => {
                self.emit_function_edge_defers(body, block, *target, *span, llvm_blocks)?;
                if self.current_block_has_terminator() {
                    return Ok(());
                }
                let target = self.llvm_function_block(*span, *target, llvm_blocks)?;
                self.builder
                    .build_unconditional_branch(target)
                    .map_err(|_| self.error(*span, "failed to build function branch"))?;
            }
            FunctionTerminator::Return { value, span } => {
                self.emit_function_return(body, block, *span, value.as_ref(), llvm_blocks)?;
            }
            FunctionTerminator::Tail { value, span } => {
                self.emit_function_tail(body, block, *span, value.as_ref(), llvm_blocks)?;
            }
            FunctionTerminator::If {
                cond,
                then_target,
                else_target,
                span,
            } => {
                let cond = self.emit_expr(cond)?.into_int_value()?;
                let then_block = self.llvm_function_block(*span, *then_target, llvm_blocks)?;
                let else_block = self.llvm_function_block(*span, *else_target, llvm_blocks)?;
                self.builder
                    .build_conditional_branch(cond, then_block, else_block)
                    .map_err(|_| self.error(*span, "failed to build function if branch"))?;
            }
            FunctionTerminator::Switch {
                target,
                arms,
                default,
                fallback,
                span,
            } => {
                let target = self.emit_expr(target)?.into_int_value()?;
                let default = default.unwrap_or(*fallback);
                let default_block = self.llvm_function_block(*span, default, llvm_blocks)?;
                let mut cases = Vec::new();
                for arm in arms {
                    let value = self.emit_switch_pattern_value(&arm.pattern)?;
                    let block = self.llvm_function_block(*span, arm.target, llvm_blocks)?;
                    cases.push((value, block));
                }
                self.builder
                    .build_switch(target, default_block, &cases)
                    .map_err(|_| self.error(*span, "failed to build function switch"))?;
            }
            FunctionTerminator::Try {
                value,
                kind,
                error_conversion,
                success_local,
                success_target,
                span,
            } => self.emit_try_terminator(TryTerminatorInput {
                body,
                block,
                span: *span,
                value,
                kind: *kind,
                error_conversion: error_conversion.as_deref(),
                success_local: *success_local,
                success_target: *success_target,
                llvm_blocks,
            })?,
            FunctionTerminator::Loop {
                header,
                body: loop_body,
                break_target,
                span,
                ..
            } => {
                let body_block = self.llvm_function_block(*span, *loop_body, llvm_blocks)?;
                let break_block = self.llvm_function_block(*span, *break_target, llvm_blocks)?;
                self.emit_function_loop_header(*span, header, body_block, break_block)?;
            }
        }
        Ok(())
    }

    fn emit_defer_function_terminator(
        &mut self,
        body: &FunctionDeferBody,
        block: FunctionBlockId,
        terminator: &FunctionTerminator,
        llvm_blocks: &std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
        outer_blocks: &std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
        defer_end: BasicBlock<'ctx>,
    ) -> Result<(), Diagnostic> {
        match terminator {
            FunctionTerminator::Error { span } => {
                self.builder
                    .build_unreachable()
                    .map_err(|_| self.error(*span, "failed to build defer function unreachable"))?;
            }
            FunctionTerminator::Next { target, span }
            | FunctionTerminator::Branch { target, span } => {
                self.emit_defer_function_edge_defers(
                    body,
                    block,
                    *target,
                    *span,
                    llvm_blocks,
                    outer_blocks,
                )?;
                if self.current_block_has_terminator() {
                    return Ok(());
                }
                let target =
                    self.llvm_defer_function_block(*span, *target, llvm_blocks, outer_blocks)?;
                self.builder
                    .build_unconditional_branch(target)
                    .map_err(|_| self.error(*span, "failed to build defer function branch"))?;
            }
            FunctionTerminator::Return { value, span } => {
                self.emit_defer_function_return(body, block, *span, value.as_ref(), outer_blocks)?;
            }
            FunctionTerminator::Tail { span, .. } => {
                self.emit_defer_function_tail_defers(body, block, *span, outer_blocks)?;
                if self.current_block_has_terminator() {
                    return Ok(());
                }
                self.builder
                    .build_unconditional_branch(defer_end)
                    .map_err(|_| self.error(*span, "failed to leave defer function body"))?;
            }
            FunctionTerminator::If {
                cond,
                then_target,
                else_target,
                span,
            } => {
                let cond = self.emit_expr(cond)?.into_int_value()?;
                let then_block =
                    self.llvm_defer_function_block(*span, *then_target, llvm_blocks, outer_blocks)?;
                let else_block =
                    self.llvm_defer_function_block(*span, *else_target, llvm_blocks, outer_blocks)?;
                self.builder
                    .build_conditional_branch(cond, then_block, else_block)
                    .map_err(|_| self.error(*span, "failed to build defer function if branch"))?;
            }
            FunctionTerminator::Switch {
                target,
                arms,
                default,
                fallback,
                span,
            } => {
                let target = self.emit_expr(target)?.into_int_value()?;
                let default = default.unwrap_or(*fallback);
                let default_block =
                    self.llvm_defer_function_block(*span, default, llvm_blocks, outer_blocks)?;
                let mut cases = Vec::new();
                for arm in arms {
                    let value = self.emit_switch_pattern_value(&arm.pattern)?;
                    let block = self.llvm_defer_function_block(
                        *span,
                        arm.target,
                        llvm_blocks,
                        outer_blocks,
                    )?;
                    cases.push((value, block));
                }
                self.builder
                    .build_switch(target, default_block, &cases)
                    .map_err(|_| self.error(*span, "failed to build defer function switch"))?;
            }
            FunctionTerminator::Try {
                value,
                kind,
                error_conversion,
                success_local,
                success_target,
                span,
            } => self.emit_defer_try_terminator(DeferTryTerminatorInput {
                body,
                block,
                span: *span,
                value,
                kind: *kind,
                error_conversion: error_conversion.as_deref(),
                success_local: *success_local,
                success_target: *success_target,
                llvm_blocks,
                outer_blocks,
            })?,
            FunctionTerminator::Loop {
                header,
                body: loop_body,
                break_target,
                span,
                ..
            } => {
                let body_block =
                    self.llvm_defer_function_block(*span, *loop_body, llvm_blocks, outer_blocks)?;
                let break_block = self.llvm_defer_function_block(
                    *span,
                    *break_target,
                    llvm_blocks,
                    outer_blocks,
                )?;
                self.emit_function_loop_header(*span, header, body_block, break_block)?;
            }
        }
        Ok(())
    }

    fn emit_defer_try_terminator(
        &mut self,
        input: DeferTryTerminatorInput<'_, 'ctx>,
    ) -> Result<(), Diagnostic> {
        let DeferTryTerminatorInput {
            body,
            block,
            span,
            value,
            kind,
            error_conversion,
            success_local,
            success_target,
            llvm_blocks,
            outer_blocks,
        } = input;
        let aggregate = self.emit_memory_operand(value)?;
        let tag = self.load_tagged_union_tag(span, value.ty, aggregate, "defer.try.tag")?;
        let failure_tag = match kind {
            FunctionTryKind::Optional => FunctionOptionalTag::Null.discriminant(),
            FunctionTryKind::ErrorUnion => FunctionErrorUnionTag::Err.discriminant(),
        };
        let failure_tag = self
            .module
            .context
            .i8_type()
            .const_int(failure_tag.into(), false)?;
        let is_failure = self
            .builder
            .build_int_compare(IntPredicate::EQ, tag, failure_tag, "defer.try.failed")
            .map_err(|_| self.error(span, "failed to compare deferred propagation tag"))?;
        let failure_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "defer.try.failure")
            .map_err(|_| self.error(span, "failed to create deferred propagation failure block"))?;
        let success_block =
            self.llvm_defer_function_block(span, success_target, llvm_blocks, outer_blocks)?;
        self.builder
            .build_conditional_branch(is_failure, failure_block, success_block)
            .map_err(|_| self.error(span, "failed to build deferred propagation branch"))?;

        self.builder.position_at_end(failure_block);
        let slot = self.return_slot(span)?;
        self.emit_try_failure_into(
            TryFailure {
                span,
                aggregate,
                aggregate_ty: value.ty,
                kind,
                error_conversion,
            },
            slot,
        )?;
        self.emit_defer_function_tail_defers(body, block, span, outer_blocks)?;
        if !self.current_block_has_terminator() {
            self.emit_memory_return(span)?;
        }

        self.builder.position_at_end(success_block);
        self.store_try_success(span, aggregate, value.ty, success_local)
    }

    fn emit_try_terminator(
        &mut self,
        input: TryTerminatorInput<'_, 'ctx>,
    ) -> Result<(), Diagnostic> {
        let TryTerminatorInput {
            body,
            block,
            span,
            value,
            kind,
            error_conversion,
            success_local,
            success_target,
            llvm_blocks,
        } = input;
        let aggregate = self.emit_memory_operand(value)?;
        let tag = self.load_tagged_union_tag(span, value.ty, aggregate, "try.tag")?;
        let failure_tag = match kind {
            FunctionTryKind::Optional => FunctionOptionalTag::Null.discriminant(),
            FunctionTryKind::ErrorUnion => FunctionErrorUnionTag::Err.discriminant(),
        };
        let failure_tag = self
            .module
            .context
            .i8_type()
            .const_int(failure_tag.into(), false)?;
        let is_failure = self
            .builder
            .build_int_compare(IntPredicate::EQ, tag, failure_tag, "try.failed")
            .map_err(|_| self.error(span, "failed to compare propagation tag"))?;
        let failure_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "try.failure")
            .map_err(|_| self.error(span, "failed to create propagation failure block"))?;
        let success_block = self.llvm_function_block(span, success_target, llvm_blocks)?;
        self.builder
            .build_conditional_branch(is_failure, failure_block, success_block)
            .map_err(|_| self.error(span, "failed to build propagation branch"))?;

        self.builder.position_at_end(failure_block);
        self.emit_try_failure_return(TryFailureReturn {
            body,
            block,
            failure: TryFailure {
                span,
                aggregate,
                aggregate_ty: value.ty,
                kind,
                error_conversion,
            },
            outer_blocks: llvm_blocks,
        })?;

        self.builder.position_at_end(success_block);
        self.store_try_success(span, aggregate, value.ty, success_local)
    }

    /// Copies the payload of a successful propagation into its local.
    fn store_try_success(
        &mut self,
        span: Span,
        aggregate: PointerValue<'ctx>,
        aggregate_ty: nia_ids::InternedTyId,
        success_local: nia_ids::LocalId,
    ) -> Result<(), Diagnostic> {
        if self.is_zero_sized_local(success_local) {
            return Ok(());
        }
        let Some(ptr) = self.locals.get(&success_local).copied() else {
            return Err(self.error(span, "missing propagation success local"));
        };
        let Some(payload_ty) = self.local_tys.get(&success_local).copied() else {
            return Err(self.error(span, "missing propagation success local type"));
        };
        let payload = self.tagged_union_payload_addr(span, aggregate_ty, aggregate)?;
        self.transfer_value(ptr, payload, payload_ty, span)
    }

    fn emit_try_failure_return(
        &mut self,
        failure: TryFailureReturn<'_, 'ctx>,
    ) -> Result<(), Diagnostic> {
        let TryFailureReturn {
            body,
            block,
            failure,
            outer_blocks,
        } = failure;
        let span = failure.span;
        let slot = self.return_slot(span)?;
        self.emit_try_failure_into(failure, slot)?;
        if self.queue_function_return_cleanup(body, block, span, None)? {
            return Ok(());
        }
        self.emit_function_tail_defers(body, block, span, outer_blocks)?;
        if self.current_block_has_terminator() {
            return Ok(());
        }
        self.emit_memory_return(span)
    }

    /// Writes the failure that a `.?` propagates into the result slot.
    fn emit_try_failure_into(
        &mut self,
        failure: TryFailure<'_, 'ctx>,
        return_ptr: PointerValue<'ctx>,
    ) -> Result<(), Diagnostic> {
        let TryFailure {
            span,
            aggregate,
            aggregate_ty,
            kind,
            error_conversion,
        } = failure;
        let return_ty = self.function.return_type;
        let return_llvm_ty = self.module.llvm_basic_type(return_ty, span)?;
        let tag = match kind {
            FunctionTryKind::Optional => FunctionOptionalTag::Null.discriminant(),
            FunctionTryKind::ErrorUnion => FunctionErrorUnionTag::Err.discriminant(),
        };
        if kind == FunctionTryKind::ErrorUnion
            && let Some(payload_ty) = self.error_union_error_ty(return_ty)
        {
            let payload_ptr = self.tagged_union_payload_addr(span, return_ty, return_ptr)?;
            match error_conversion {
                Some(conversion) => self.emit_into(conversion, payload_ptr)?,
                None => {
                    let payload = self.tagged_union_payload_addr(span, aggregate_ty, aggregate)?;
                    self.transfer_value(payload_ptr, payload, payload_ty, span)?;
                }
            }
        } else if let Some(conversion) = error_conversion {
            self.emit_effect_expr(conversion)?;
        }
        let tag_ptr = unsafe {
            self.builder
                .build_struct_gep(return_llvm_ty, return_ptr, 0, "try.failure.tag")
        }
        .map_err(|_| self.error(span, "failed to build propagation return tag"))?;
        let tag = self.module.context.i8_type().const_int(tag.into(), false)?;
        self.builder
            .build_store(tag_ptr, tag)
            .map_err(|_| self.error(span, "failed to store propagation return tag"))?;
        Ok(())
    }

    fn emit_function_loop_header(
        &mut self,
        span: Span,
        header: &FunctionForHeader,
        body_block: BasicBlock<'ctx>,
        break_block: BasicBlock<'ctx>,
    ) -> Result<(), Diagnostic> {
        match header {
            FunctionForHeader::Infinite => {
                self.builder
                    .build_unconditional_branch(body_block)
                    .map_err(|_| self.error(span, "failed to build function loop branch"))?;
            }
            FunctionForHeader::Condition(cond) => {
                let cond = self.emit_expr(cond)?.into_int_value()?;
                self.builder
                    .build_conditional_branch(cond, body_block, break_block)
                    .map_err(|_| self.error(span, "failed to build function loop branch"))?;
            }
        }
        Ok(())
    }

    fn emit_function_edge_defers(
        &mut self,
        body: &FunctionBody,
        from: FunctionBlockId,
        to: FunctionBlockId,
        span: Span,
        outer_blocks: &std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
    ) -> Result<(), Diagnostic> {
        let Some(scopes) = body.edge_exited_scopes(from, to) else {
            return Err(self.error(span, "invalid function branch scopes"));
        };
        for scope in scopes {
            self.emit_function_scope_defers(span, scope, outer_blocks)?;
        }
        Ok(())
    }

    fn emit_function_return(
        &mut self,
        body: &FunctionBody,
        block: FunctionBlockId,
        span: Span,
        value: Option<&FunctionExpr>,
        outer_blocks: &std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
    ) -> Result<(), Diagnostic> {
        if let Some(value) = value {
            if self.is_zero_sized(value.ty) {
                self.emit_effect_expr(value)?;
                if self.current_block_has_terminator() {
                    return Ok(());
                }
                if self.queue_function_return_cleanup(body, block, span, None)? {
                    return Ok(());
                }
                self.emit_function_tail_defers(body, block, span, outer_blocks)?;
                if self.current_block_has_terminator() {
                    return Ok(());
                }
                if self.is_never(self.function.return_type) {
                    self.builder
                        .build_unreachable()
                        .map_err(|_| self.error(span, "failed to build never return"))?;
                } else {
                    self.builder
                        .build_return(None)
                        .map_err(|_| self.error(span, "failed to build void return"))?;
                }
                return Ok(());
            }
            let value = self.emit_returned_value(span, value)?;
            if self.current_block_has_terminator() {
                return Ok(());
            }
            if self.queue_function_return_cleanup(body, block, span, value)? {
                return Ok(());
            }
            self.emit_function_tail_defers(body, block, span, outer_blocks)?;
            if self.current_block_has_terminator() {
                return Ok(());
            }
            if self.is_never(self.function.return_type) {
                self.builder
                    .build_unreachable()
                    .map_err(|_| self.error(span, "failed to build never return"))?;
            } else {
                self.emit_returned(span, value)?;
            }
        } else {
            if self.queue_function_return_cleanup(body, block, span, None)? {
                return Ok(());
            }
            self.emit_function_tail_defers(body, block, span, outer_blocks)?;
            if self.current_block_has_terminator() {
                return Ok(());
            }
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

    fn emit_function_tail(
        &mut self,
        body: &FunctionBody,
        block: FunctionBlockId,
        span: Span,
        value: Option<&FunctionExpr>,
        outer_blocks: &std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
    ) -> Result<(), Diagnostic> {
        let Some(value) = value else {
            if self.is_never(body.ty) {
                self.builder
                    .build_unreachable()
                    .map_err(|_| self.error(span, "failed to build diverging body unreachable"))?;
            } else if self.is_unit(self.function.return_type) {
                if self.queue_function_return_cleanup(body, block, span, None)? {
                    return Ok(());
                }
                self.emit_function_tail_defers(body, block, span, outer_blocks)?;
                if self.current_block_has_terminator() {
                    return Ok(());
                }
                self.builder
                    .build_return(None)
                    .map_err(|_| self.error(span, "failed to build void return"))?;
            } else if self.is_never(self.function.return_type) {
                if self.queue_function_return_cleanup(body, block, span, None)? {
                    return Ok(());
                }
                self.emit_function_tail_defers(body, block, span, outer_blocks)?;
                if self.current_block_has_terminator() {
                    return Ok(());
                }
                self.builder
                    .build_unreachable()
                    .map_err(|_| self.error(span, "failed to build never function unreachable"))?;
            }
            return Ok(());
        };
        if self.is_zero_sized(value.ty) {
            self.emit_effect_expr(value)?;
            if self.current_block_has_terminator() {
                return Ok(());
            }
            if self.queue_function_return_cleanup(body, block, span, None)? {
                return Ok(());
            }
            self.emit_function_tail_defers(body, block, span, outer_blocks)?;
            if self.current_block_has_terminator() {
                return Ok(());
            }
            self.builder
                .build_return(None)
                .map_err(|_| self.error(span, "failed to build void return"))?;
            return Ok(());
        }
        let value = self.emit_returned_value(span, value)?;
        if self.current_block_has_terminator() {
            return Ok(());
        }
        if self.queue_function_return_cleanup(body, block, span, value)? {
            return Ok(());
        }
        self.emit_function_tail_defers(body, block, span, outer_blocks)?;
        if self.current_block_has_terminator() {
            return Ok(());
        }
        self.emit_returned(span, value)
    }

    /// Evaluates a returned value: a memory value goes straight into the
    /// result slot, and an immediate one becomes an SSA value.
    fn emit_returned_value(
        &mut self,
        span: Span,
        value: &FunctionExpr,
    ) -> Result<Option<BasicValueEnum<'ctx>>, Diagnostic> {
        if !self.is_memory_value(value.ty) {
            return self.emit_expr(value).map(Some);
        }
        let slot = self.return_slot(span)?;
        self.emit_into(value, slot)?;
        Ok(None)
    }

    /// Returns what [`Self::emit_returned_value`] produced.
    fn emit_returned(
        &mut self,
        span: Span,
        value: Option<BasicValueEnum<'ctx>>,
    ) -> Result<(), Diagnostic> {
        match value {
            Some(value) => self.emit_return_value(span, value),
            None => self.emit_memory_return(span),
        }
    }

    fn queue_function_return_cleanup(
        &mut self,
        body: &FunctionBody,
        block: FunctionBlockId,
        span: Span,
        value: Option<BasicValueEnum<'ctx>>,
    ) -> Result<bool, Diagnostic> {
        let Some(exited_scopes) = body.return_exited_scopes(block) else {
            return Err(self.error(span, "invalid function return scopes"));
        };
        let mut cleanup_bodies = Vec::new();
        for scope_id in exited_scopes {
            let Some(index) = self.function_defer_scopes.get(&scope_id).copied() else {
                return Err(self.error(span, "missing function defer scope"));
            };
            let Some(scope) = self.defer_scopes.get(index) else {
                return Err(self.error(span, "missing function defer storage"));
            };
            for (body_index, defer_body) in scope.bodies.iter().enumerate().rev() {
                cleanup_bodies.push(((scope_id, body_index), defer_body.clone()));
            }
        }
        if cleanup_bodies.is_empty() {
            return Ok(false);
        }

        if let Some(value) = value {
            let Some(storage) = self.function_return_storage else {
                return Err(self.error(span, "missing cleanup return storage"));
            };
            self.builder
                .build_store(storage, value)
                .map_err(|_| self.error(span, "failed to store cleanup return value"))?;
        }

        let mut next = if let Some(entry) = self.function_return_cleanup_return {
            entry
        } else {
            let entry = self
                .module
                .context
                .append_basic_block(self.llvm_function, "return.cleanup")?;
            self.function_return_cleanup_return = Some(entry);
            self.pending_function_return_cleanups
                .push(super::FunctionReturnCleanup {
                    entry,
                    body: None,
                    next: None,
                    span,
                });
            entry
        };

        for ((scope_id, body_index), defer_body) in cleanup_bodies.into_iter().rev() {
            let key = (scope_id, body_index, next);
            if let Some(entry) = self.function_return_cleanup_blocks.get(&key).copied() {
                next = entry;
                continue;
            }
            let entry = self
                .module
                .context
                .append_basic_block(self.llvm_function, "return.cleanup")?;
            self.function_return_cleanup_blocks.insert(key, entry);
            self.pending_function_return_cleanups
                .push(super::FunctionReturnCleanup {
                    entry,
                    body: Some(defer_body),
                    next: Some(next),
                    span,
                });
            next = entry;
        }
        self.builder
            .build_unconditional_branch(next)
            .map_err(|_| self.error(span, "failed to branch to function return cleanup"))?;
        Ok(true)
    }

    fn emit_pending_function_return_cleanups(
        &mut self,
        outer_blocks: &std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
    ) -> Result<(), Diagnostic> {
        let cleanups = std::mem::take(&mut self.pending_function_return_cleanups);
        for cleanup in cleanups.into_iter().rev() {
            self.builder.position_at_end(cleanup.entry);
            if let Some(body) = cleanup.body {
                self.emit_defer_function_body(&body, outer_blocks)?;
                if self.current_block_has_terminator() {
                    continue;
                }
                let Some(next) = cleanup.next else {
                    return Err(self.error(cleanup.span, "missing cleanup successor"));
                };
                self.builder.build_unconditional_branch(next).map_err(|_| {
                    self.error(cleanup.span, "failed to branch to next function cleanup")
                })?;
                continue;
            }
            let mut last_block = cleanup.entry;
            while let Some(next) = last_block.get_next_basic_block() {
                last_block = next;
            }
            cleanup
                .entry
                .move_after(last_block)
                .map_err(super::ModuleCodegen::diagnostic_from_llvm_error)?;
            self.builder.position_at_end(cleanup.entry);
            self.emit_stored_function_return(cleanup.span)?;
        }
        Ok(())
    }

    fn emit_stored_function_return(&mut self, span: Span) -> Result<(), Diagnostic> {
        match self
            .module
            .classify_function_return(self.function.return_type)
        {
            crate::module_codegen::AbiReturn::IndirectOut(_) => self.emit_memory_return(span)?,
            crate::module_codegen::AbiReturn::Direct(_) => {
                let Some(storage) = self.function_return_storage else {
                    return Err(self.error(span, "missing cleanup return storage"));
                };
                let ty = self
                    .module
                    .llvm_basic_type(self.function.return_type, span)?;
                let value = self
                    .builder
                    .build_load(ty, storage, "return.cleanup.value")
                    .map_err(|_| self.error(span, "failed to load cleanup return value"))?;
                self.emit_return_value(span, value)?;
            }
            crate::module_codegen::AbiReturn::Void => {
                self.builder
                    .build_return(None)
                    .map_err(|_| self.error(span, "failed to build cleanup return"))?;
            }
            crate::module_codegen::AbiReturn::Never => {
                self.builder
                    .build_unreachable()
                    .map_err(|_| self.error(span, "failed to build cleanup never return"))?;
            }
        }
        Ok(())
    }

    fn emit_function_tail_defers(
        &mut self,
        body: &FunctionBody,
        block: FunctionBlockId,
        span: Span,
        outer_blocks: &std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
    ) -> Result<(), Diagnostic> {
        let Some(scopes) = body.return_exited_scopes(block) else {
            return Err(self.error(span, "invalid function tail scopes"));
        };
        for scope in scopes {
            self.emit_function_scope_defers(span, scope, outer_blocks)?;
        }
        Ok(())
    }

    fn emit_defer_function_edge_defers(
        &mut self,
        body: &FunctionDeferBody,
        from: FunctionBlockId,
        to: FunctionBlockId,
        span: Span,
        llvm_blocks: &std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
        outer_blocks: &std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
    ) -> Result<(), Diagnostic> {
        let scopes = if llvm_blocks.contains_key(&to) {
            body.edge_exited_scopes(from, to)
        } else if outer_blocks.contains_key(&to) {
            body.return_exited_scopes(from)
        } else {
            None
        };
        let Some(scopes) = scopes else {
            return Err(self.error(span, "invalid defer function branch scopes"));
        };
        for scope in scopes {
            self.emit_function_scope_defers(span, scope, outer_blocks)?;
        }
        Ok(())
    }

    fn emit_defer_function_tail_defers(
        &mut self,
        body: &FunctionDeferBody,
        block: FunctionBlockId,
        span: Span,
        outer_blocks: &std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
    ) -> Result<(), Diagnostic> {
        let Some(scopes) = body.return_exited_scopes(block) else {
            return Err(self.error(span, "invalid defer function tail scopes"));
        };
        for scope in scopes {
            self.emit_function_scope_defers(span, scope, outer_blocks)?;
        }
        Ok(())
    }

    fn emit_defer_function_return(
        &mut self,
        body: &FunctionDeferBody,
        block: FunctionBlockId,
        span: Span,
        value: Option<&FunctionExpr>,
        outer_blocks: &std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
    ) -> Result<(), Diagnostic> {
        if let Some(value) = value {
            if self.is_zero_sized(value.ty) {
                self.emit_effect_expr(value)?;
                if self.current_block_has_terminator() {
                    return Ok(());
                }
                self.emit_defer_function_tail_defers(body, block, span, outer_blocks)?;
                if self.current_block_has_terminator() {
                    return Ok(());
                }
                return if self.is_never(self.function.return_type) {
                    self.builder
                        .build_unreachable()
                        .map_err(|_| self.error(span, "failed to build deferred never return"))?;
                    Ok(())
                } else {
                    self.builder
                        .build_return(None)
                        .map_err(|_| self.error(span, "failed to build deferred void return"))?;
                    Ok(())
                };
            }
            let value = self.emit_returned_value(span, value)?;
            if self.current_block_has_terminator() {
                return Ok(());
            }
            self.emit_defer_function_tail_defers(body, block, span, outer_blocks)?;
            if self.current_block_has_terminator() {
                return Ok(());
            }
            return self.emit_returned(span, value);
        }

        self.emit_defer_function_tail_defers(body, block, span, outer_blocks)?;
        if self.current_block_has_terminator() {
            return Ok(());
        }
        if self.is_never(self.function.return_type) {
            self.builder
                .build_unreachable()
                .map_err(|_| self.error(span, "failed to build deferred never return"))?;
        } else {
            self.builder
                .build_return(None)
                .map_err(|_| self.error(span, "failed to build deferred void return"))?;
        }
        Ok(())
    }

    fn llvm_function_block(
        &self,
        span: Span,
        id: FunctionBlockId,
        llvm_blocks: &std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
    ) -> Result<BasicBlock<'ctx>, Diagnostic> {
        llvm_blocks
            .get(&id)
            .copied()
            .ok_or_else(|| self.error(span, "missing function branch target"))
    }

    fn llvm_defer_function_block(
        &self,
        span: Span,
        id: FunctionBlockId,
        llvm_blocks: &std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
        outer_blocks: &std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
    ) -> Result<BasicBlock<'ctx>, Diagnostic> {
        llvm_blocks
            .get(&id)
            .or_else(|| outer_blocks.get(&id))
            .copied()
            .ok_or_else(|| self.error(span, "missing defer function branch target"))
    }

    pub(super) fn emit_switch_pattern_value(
        &mut self,
        pattern: &FunctionExpr,
    ) -> Result<IntValue<'ctx>, Diagnostic> {
        match &pattern.kind {
            FunctionExprKind::Null => {
                return Ok(self
                    .module
                    .context
                    .i8_type()
                    .const_int(FunctionOptionalTag::Null.discriminant().into(), false)?);
            }
            FunctionExprKind::OptionalSome { .. } => {
                return Ok(self
                    .module
                    .context
                    .i8_type()
                    .const_int(FunctionOptionalTag::Some.discriminant().into(), false)?);
            }
            FunctionExprKind::ErrorOk { .. } => {
                return Ok(self
                    .module
                    .context
                    .i8_type()
                    .const_int(FunctionErrorUnionTag::Ok.discriminant().into(), false)?);
            }
            FunctionExprKind::ErrorErr { .. } => {
                return Ok(self
                    .module
                    .context
                    .i8_type()
                    .const_int(FunctionErrorUnionTag::Err.discriminant().into(), false)?);
            }
            _ => {}
        }
        Ok(self.emit_expr(pattern)?.into_int_value()?)
    }

    pub(super) fn emit_binding(
        &mut self,
        span: Span,
        binding: &FunctionBinding,
    ) -> Result<(), Diagnostic> {
        if let Some(value) = &binding.value {
            if self.is_zero_sized(binding.ty) {
                return self.emit_effect_expr(value);
            }
            let Some(ptr) = self.locals.get(&binding.local_id).copied() else {
                return Err(self.error(span, "missing local binding storage"));
            };
            self.emit_into(value, ptr)?;
        }
        Ok(())
    }

    fn emit_store_local(
        &mut self,
        span: Span,
        local_id: nia_ids::LocalId,
        value: &FunctionExpr,
    ) -> Result<(), Diagnostic> {
        if self.is_zero_sized(value.ty) {
            return self.emit_effect_expr(value);
        }
        let Some(ptr) = self.locals.get(&local_id).copied() else {
            return Err(self.error(span, "missing local binding storage"));
        };
        self.emit_into(value, ptr)
    }

    pub(super) fn emit_effect_expr(&mut self, expr: &FunctionExpr) -> Result<(), Diagnostic> {
        match &expr.kind {
            FunctionExprKind::Assign { place, op, rhs } => {
                if self.is_memory_value(rhs.ty)
                    && !matches!(place.base, nia_function_ir::FunctionPlaceBase::Error)
                {
                    return self.emit_memory_assign(expr.span, place, rhs);
                }
                if self.is_zero_sized(rhs.ty) {
                    self.emit_effect_expr(rhs)?;
                    if self.current_block_has_terminator() {
                        return Ok(());
                    }
                    return Ok(());
                }
                let rhs_ty = rhs.ty;
                let value = self.emit_expr(rhs)?;
                self.emit_assign(expr.span, place, *op, rhs_ty, value)
            }
            FunctionExprKind::Discard(inner) => self.emit_effect_expr(inner),
            FunctionExprKind::Call { callee, args } => {
                if self.is_memory_value(expr.ty) {
                    let result = self.temporary(expr.ty, "discard.call.out", expr.span)?;
                    self.emit_call_into(expr, callee, args, result)
                } else {
                    self.emit_call_effect(expr, callee, args)
                }
            }
            FunctionExprKind::Trap => self.emit_trap(expr.span),
            FunctionExprKind::InlineAsm(asm) => self.emit_inline_asm(asm),
            FunctionExprKind::Atomic(atomic) => self.emit_atomic_effect(expr, atomic),
            FunctionExprKind::ArrayLiteral { elems } => self.emit_array_literal_effects(elems),
            FunctionExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    self.emit_effect_expr(&field.value)?;
                }
                Ok(())
            }
            FunctionExprKind::Tuple(elems)
            | FunctionExprKind::EnumVariant { fields: elems, .. } => {
                for elem in elems {
                    self.emit_effect_expr(elem)?;
                }
                Ok(())
            }
            FunctionExprKind::UnionLiteral { field, .. } => self.emit_effect_expr(&field.value),
            FunctionExprKind::UnionStorageLiteral { .. } => Ok(()),
            FunctionExprKind::OptionalSome { expr }
            | FunctionExprKind::ErrorOk { expr }
            | FunctionExprKind::ErrorErr { expr }
            | FunctionExprKind::TaggedUnionTag { expr }
            | FunctionExprKind::TaggedUnionPayload { expr }
            | FunctionExprKind::Try { expr } => self.emit_effect_expr(expr),
            FunctionExprKind::Local(_)
            | FunctionExprKind::Global(_)
            | FunctionExprKind::Function(_)
            | FunctionExprKind::FunctionInstance { .. }
            | FunctionExprKind::Null
            | FunctionExprKind::StaticArrayPointer { .. } => Ok(()),
            _ if self.is_memory_value(expr.ty) => self.emit_memory_operand(expr).map(|_| ()),
            _ => {
                let _ = self.emit_expr(expr)?;
                Ok(())
            }
        }
    }

    fn emit_array_literal_effects(
        &mut self,
        elems: &nia_function_ir::FunctionArrayElements,
    ) -> Result<(), Diagnostic> {
        match elems {
            nia_function_ir::FunctionArrayElements::List(values) => {
                for value in values {
                    self.emit_effect_expr(value)?;
                }
            }
            nia_function_ir::FunctionArrayElements::Repeat { value, count } => {
                let count = self.module.array_len(count, value.span)?;
                self.emit_const_count_loop(value.span, count, "array.repeat.effect", |this, _| {
                    this.emit_effect_expr(value)
                })?;
            }
        }
        Ok(())
    }
}

fn function_ir_diagnostic(error: FunctionIrError) -> Diagnostic {
    Diagnostic::internal_error(
        nia_diagnostic::codes::INVALID_FUNCTION_IR,
        "invalid function IR before LLVM codegen",
    )
    .primary(error.span, "function IR validation failed here")
    .debug("function_ir_error", error.message)
    .finish()
}
