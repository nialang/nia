// SPDX-License-Identifier: GPL-3.0-or-later
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    FunctionBinding, FunctionBlockId, FunctionBody, FunctionDeferBody, FunctionErrorUnionTag,
    FunctionExpr, FunctionExprKind, FunctionForHeader, FunctionIrError, FunctionOp,
    FunctionOptionalTag, FunctionScopeId, FunctionTerminator, FunctionTryKind,
    validate_function_body,
};
use nia_llvm::{IntPredicate, basic_block::BasicBlock, values::IntValue};
use nia_span::Span;

use super::{FunctionCodegen, callee_is_extern};

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

struct TryFailureReturn<'b, 'ctx> {
    body: &'b FunctionBody,
    block: FunctionBlockId,
    span: Span,
    aggregate: nia_llvm::values::StructValue<'ctx>,
    aggregate_ty: nia_ids::InternedTyId,
    kind: FunctionTryKind,
    error_conversion: Option<&'b FunctionExpr>,
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
        let aggregate = self.emit_tagged_union_value(span, value)?;
        let tag = self
            .builder
            .build_extract_value(aggregate, 0, "defer.try.tag")
            .map_err(|_| self.error(span, "failed to extract deferred propagation tag"))?
            .into_int_value()?;
        let failure_tag = match kind {
            FunctionTryKind::Optional => FunctionOptionalTag::Null.discriminant(),
            FunctionTryKind::ErrorUnion => FunctionErrorUnionTag::Err.discriminant(),
        };
        let failure_tag = self
            .module
            .context
            .i8_type()
            .const_int(failure_tag.into(), false);
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
        let (return_llvm_ty, return_ptr) = self.emit_try_failure_return_storage(
            span,
            aggregate,
            value.ty,
            kind,
            error_conversion,
        )?;
        self.emit_defer_function_tail_defers(body, block, span, outer_blocks)?;
        if self.current_block_has_terminator() {
            return Ok(());
        }
        let return_value = self
            .builder
            .build_load(return_llvm_ty, return_ptr, "try.return.value")
            .map_err(|_| self.error(span, "failed to load propagation return"))?;
        self.emit_return_value(span, return_value)?;

        self.builder.position_at_end(success_block);
        if !self.is_zero_sized_local(success_local) {
            let Some(ptr) = self.locals.get(&success_local).copied() else {
                return Err(self.error(span, "missing deferred propagation success local"));
            };
            let Some(payload_ty) = self.local_tys.get(&success_local).copied() else {
                return Err(self.error(span, "missing deferred propagation success local type"));
            };
            let payload = self.load_tagged_union_payload_from_value(
                span,
                aggregate.into(),
                value.ty,
                payload_ty,
            )?;
            self.builder
                .build_store(ptr, payload)
                .map_err(|_| self.error(span, "failed to store deferred propagation payload"))?;
        }
        Ok(())
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
        let aggregate = self.emit_tagged_union_value(span, value)?;
        let tag = self
            .builder
            .build_extract_value(aggregate, 0, "try.tag")
            .map_err(|_| self.error(span, "failed to extract propagation tag"))?
            .into_int_value()?;
        let failure_tag = match kind {
            FunctionTryKind::Optional => FunctionOptionalTag::Null.discriminant(),
            FunctionTryKind::ErrorUnion => FunctionErrorUnionTag::Err.discriminant(),
        };
        let failure_tag = self
            .module
            .context
            .i8_type()
            .const_int(failure_tag.into(), false);
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
            span,
            aggregate,
            aggregate_ty: value.ty,
            kind,
            error_conversion,
            outer_blocks: llvm_blocks,
        })?;

        self.builder.position_at_end(success_block);
        if !self.is_zero_sized_local(success_local) {
            let Some(ptr) = self.locals.get(&success_local).copied() else {
                return Err(self.error(span, "missing propagation success local"));
            };
            let Some(payload_ty) = self.local_tys.get(&success_local).copied() else {
                return Err(self.error(span, "missing propagation success local type"));
            };
            let payload = self.load_tagged_union_payload_from_value(
                span,
                aggregate.into(),
                value.ty,
                payload_ty,
            )?;
            self.builder
                .build_store(ptr, payload)
                .map_err(|_| self.error(span, "failed to store propagation payload"))?;
        }
        Ok(())
    }

    fn emit_try_failure_return(
        &mut self,
        failure: TryFailureReturn<'_, 'ctx>,
    ) -> Result<(), Diagnostic> {
        let TryFailureReturn {
            body,
            block,
            span,
            aggregate,
            aggregate_ty,
            kind,
            error_conversion,
            outer_blocks,
        } = failure;
        let (return_llvm_ty, return_ptr) = self.emit_try_failure_return_storage(
            span,
            aggregate,
            aggregate_ty,
            kind,
            error_conversion,
        )?;
        self.emit_function_tail_defers(body, block, span, outer_blocks)?;
        if self.current_block_has_terminator() {
            return Ok(());
        }
        let value = self
            .builder
            .build_load(return_llvm_ty, return_ptr, "try.return.value")
            .map_err(|_| self.error(span, "failed to load propagation return"))?;
        self.emit_return_value(span, value)
    }

    pub(super) fn emit_tagged_union_value(
        &mut self,
        span: Span,
        value: &FunctionExpr,
    ) -> Result<nia_llvm::values::StructValue<'ctx>, Diagnostic> {
        if matches!(
            self.module.classify_function_return(value.ty),
            crate::module_codegen::AbiReturn::IndirectOut(_)
        ) {
            let value_ty = self.module.llvm_basic_type(value.ty, value.span)?;
            let ptr = self
                .builder
                .build_alloca(value_ty, "tagged.union.tmp")
                .map_err(|_| self.error(span, "failed to allocate tagged union value"))?;
            if !self.emit_aggregate_literal_into(ptr, value)?
                && !self.emit_aggregate_call_result_into(ptr, value)?
            {
                let emitted = self.emit_expr(value)?;
                self.builder
                    .build_store(ptr, emitted)
                    .map_err(|_| self.error(span, "failed to store tagged union value"))?;
            }
            return self
                .builder
                .build_load(value_ty, ptr, "tagged.union")
                .map_err(|_| self.error(span, "failed to load tagged union value"))?
                .into_struct_value()
                .map_err(|_| self.error(span, "tagged union value is not a struct"));
        }
        self.emit_expr(value)?
            .into_struct_value()
            .map_err(|_| self.error(span, "tagged union value is not a struct"))
    }

    fn emit_try_failure_return_storage(
        &mut self,
        span: Span,
        aggregate: nia_llvm::values::StructValue<'ctx>,
        aggregate_ty: nia_ids::InternedTyId,
        kind: FunctionTryKind,
        error_conversion: Option<&FunctionExpr>,
    ) -> Result<
        (
            nia_llvm::types::BasicTypeEnum<'ctx>,
            nia_llvm::values::PointerValue<'ctx>,
        ),
        Diagnostic,
    > {
        let return_ty = self.function.return_type;
        let return_llvm_ty = self.module.llvm_basic_type(return_ty, span)?;
        let return_ptr = self
            .builder
            .build_alloca(return_llvm_ty, "try.return")
            .map_err(|_| self.error(span, "failed to allocate propagation return"))?;
        match kind {
            FunctionTryKind::Optional => {
                let expr = FunctionExpr {
                    span,
                    ty: return_ty,
                    kind: FunctionExprKind::Null,
                };
                self.emit_tagged_union_into(
                    &expr,
                    FunctionOptionalTag::Null.discriminant(),
                    None,
                    return_ptr,
                )?;
            }
            FunctionTryKind::ErrorUnion => {
                let converted_payload = if let Some(conversion) = error_conversion {
                    if self.is_zero_sized(conversion.ty) {
                        self.emit_effect_expr(conversion)?;
                        None
                    } else {
                        Some(self.emit_expr(conversion)?)
                    }
                } else {
                    None
                };
                let tag_ptr = self
                    .builder
                    .build_struct_gep(return_llvm_ty, return_ptr, 0, "try.return.tag")
                    .map_err(|_| self.error(span, "failed to build propagation return tag"))?;
                let tag = self
                    .module
                    .context
                    .i8_type()
                    .const_int(FunctionErrorUnionTag::Err.discriminant().into(), false);
                self.builder
                    .build_store(tag_ptr, tag)
                    .map_err(|_| self.error(span, "failed to store propagation return tag"))?;
                if let Some(payload_ty) = self.error_union_error_ty(return_ty)
                    && !self.is_zero_sized(payload_ty)
                {
                    let payload_ptr = self
                        .builder
                        .build_struct_gep(return_llvm_ty, return_ptr, 1, "try.return.payload")
                        .map_err(|_| {
                            self.error(span, "failed to build propagation return payload")
                        })?;
                    let payload = match converted_payload {
                        Some(payload) => payload,
                        None => self.load_tagged_union_payload_from_value(
                            span,
                            aggregate.into(),
                            aggregate_ty,
                            payload_ty,
                        )?,
                    };
                    self.builder
                        .build_store(payload_ptr, payload)
                        .map_err(|_| {
                            self.error(span, "failed to store propagation return payload")
                        })?;
                }
            }
        }
        Ok((return_llvm_ty, return_ptr))
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
            if self.emit_indirect_aggregate_literal_return(
                body,
                block,
                span,
                value,
                outer_blocks,
            )? {
                return Ok(());
            }
            if self.emit_indirect_aggregate_call_return(body, block, span, value)? {
                return Ok(());
            }
            let value = self.emit_expr(value)?;
            self.emit_function_tail_defers(body, block, span, outer_blocks)?;
            if self.current_block_has_terminator() {
                return Ok(());
            }
            if self.is_never(self.function.return_type) {
                self.builder
                    .build_unreachable()
                    .map_err(|_| self.error(span, "failed to build never return"))?;
            } else {
                self.emit_return_value(span, value)?;
            }
        } else {
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
            if self.is_unit(self.function.return_type) {
                self.emit_function_tail_defers(body, block, span, outer_blocks)?;
                if self.current_block_has_terminator() {
                    return Ok(());
                }
                self.builder
                    .build_return(None)
                    .map_err(|_| self.error(span, "failed to build void return"))?;
            } else if self.is_never(self.function.return_type) {
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
            self.emit_function_tail_defers(body, block, span, outer_blocks)?;
            if self.current_block_has_terminator() {
                return Ok(());
            }
            self.builder
                .build_return(None)
                .map_err(|_| self.error(span, "failed to build void return"))?;
            return Ok(());
        }
        if self.emit_indirect_aggregate_literal_return(body, block, span, value, outer_blocks)? {
            return Ok(());
        }
        if self.emit_indirect_aggregate_call_return(body, block, span, value)? {
            return Ok(());
        }
        let value = self.emit_expr(value)?;
        if self.current_block_has_terminator() {
            return Ok(());
        }
        self.emit_function_tail_defers(body, block, span, outer_blocks)?;
        if self.current_block_has_terminator() {
            return Ok(());
        }
        self.emit_return_value(span, value)
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
            let value = self.emit_expr(value)?;
            if self.current_block_has_terminator() {
                return Ok(());
            }
            self.emit_defer_function_tail_defers(body, block, span, outer_blocks)?;
            if self.current_block_has_terminator() {
                return Ok(());
            }
            return self.emit_return_value(span, value);
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
                    .const_int(FunctionOptionalTag::Null.discriminant().into(), false));
            }
            FunctionExprKind::OptionalSome { .. } => {
                return Ok(self
                    .module
                    .context
                    .i8_type()
                    .const_int(FunctionOptionalTag::Some.discriminant().into(), false));
            }
            FunctionExprKind::ErrorOk { .. } => {
                return Ok(self
                    .module
                    .context
                    .i8_type()
                    .const_int(FunctionErrorUnionTag::Ok.discriminant().into(), false));
            }
            FunctionExprKind::ErrorErr { .. } => {
                return Ok(self
                    .module
                    .context
                    .i8_type()
                    .const_int(FunctionErrorUnionTag::Err.discriminant().into(), false));
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
                self.emit_effect_expr(value)?;
                if self.current_block_has_terminator() {
                    return Ok(());
                }
                return Ok(());
            }
            let Some(ptr) = self.locals.get(&binding.local_id).copied() else {
                return Err(self.error(span, "missing local binding storage"));
            };
            self.emit_store_value(span, ptr, value, false)?;
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
            self.emit_effect_expr(value)?;
            if self.current_block_has_terminator() {
                return Ok(());
            }
            return Ok(());
        }
        let Some(ptr) = self.locals.get(&local_id).copied() else {
            return Err(self.error(span, "missing local binding storage"));
        };
        self.emit_store_value(span, ptr, value, false)?;
        Ok(())
    }

    fn emit_store_value(
        &mut self,
        span: Span,
        ptr: nia_llvm::values::PointerValue<'ctx>,
        value: &FunctionExpr,
        is_volatile: bool,
    ) -> Result<(), Diagnostic> {
        if self.is_zero_sized(value.ty) {
            self.emit_effect_expr(value)?;
            if self.current_block_has_terminator() {
                return Ok(());
            }
            return Ok(());
        }
        if self.emit_aggregate_literal_into(ptr, value)? {
            return Ok(());
        }
        if self.emit_aggregate_call_result_into(ptr, value)? {
            return Ok(());
        }
        let value = self.emit_expr(value)?;
        self.build_place_store(ptr, value, is_volatile)
            .map_err(|_| self.error(span, "failed to store local binding"))?;
        Ok(())
    }

    fn emit_direct_store_expr(
        &mut self,
        span: Span,
        place: &nia_function_ir::FunctionPlace,
        value: &FunctionExpr,
    ) -> Result<bool, Diagnostic> {
        if !self.is_direct_store_candidate(value) {
            return Ok(false);
        }
        let ptr = self.emit_typed_place_addr(place)?;
        let is_volatile = self.place_access_is_volatile(place);
        self.emit_store_value(span, ptr, value, is_volatile)?;
        Ok(true)
    }

    fn emit_indirect_aggregate_literal_return(
        &mut self,
        body: &FunctionBody,
        block: FunctionBlockId,
        span: Span,
        value: &FunctionExpr,
        outer_blocks: &std::collections::HashMap<FunctionBlockId, BasicBlock<'ctx>>,
    ) -> Result<bool, Diagnostic> {
        if self.out_ptr.is_none()
            || self.is_never(self.function.return_type)
            || !is_aggregate_literal(value)
        {
            return Ok(false);
        }
        let ty = self.module.llvm_basic_type(value.ty, value.span)?;
        let return_copy = self
            .builder
            .build_alloca(ty, "return.copy")
            .map_err(|_| self.error(span, "failed to allocate aggregate return"))?;
        self.emit_aggregate_literal_into(return_copy, value)?;
        self.emit_function_tail_defers(body, block, span, outer_blocks)?;
        if self.current_block_has_terminator() {
            return Ok(true);
        }
        let value = self
            .builder
            .build_load(ty, return_copy, "return.value")
            .map_err(|_| self.error(span, "failed to load aggregate return"))?;
        self.emit_return_value(span, value)?;
        Ok(true)
    }

    fn emit_indirect_aggregate_call_return(
        &mut self,
        body: &FunctionBody,
        block: FunctionBlockId,
        span: Span,
        value: &FunctionExpr,
    ) -> Result<bool, Diagnostic> {
        let FunctionExprKind::Call { callee, args } = &value.kind else {
            return Ok(false);
        };
        let Some(out_ptr) = self.out_ptr else {
            return Ok(false);
        };
        if self.is_never(self.function.return_type)
            || !matches!(
                self.module.classify_function_return(value.ty),
                crate::module_codegen::AbiReturn::IndirectOut(_)
            )
            || callee_is_extern(self, callee)
            || self.return_path_has_registered_defers(body, block, span)?
        {
            return Ok(false);
        }
        let _ = self.emit_call_raw_with_out(value, callee, args, Some(out_ptr))?;
        self.builder
            .build_return(None)
            .map_err(|_| self.error(span, "failed to build aggregate return"))?;
        Ok(true)
    }

    fn return_path_has_registered_defers(
        &self,
        body: &FunctionBody,
        block: FunctionBlockId,
        span: Span,
    ) -> Result<bool, Diagnostic> {
        let Some(scopes) = body.return_exited_scopes(block) else {
            return Err(self.error(span, "invalid function tail scopes"));
        };
        Ok(scopes.into_iter().any(|scope| {
            self.function_defer_scopes
                .get(&scope)
                .and_then(|index| self.defer_scopes.get(*index))
                .is_some_and(|scope| !scope.bodies.is_empty())
        }))
    }

    fn emit_aggregate_literal_into(
        &mut self,
        ptr: nia_llvm::values::PointerValue<'ctx>,
        value: &FunctionExpr,
    ) -> Result<bool, Diagnostic> {
        match &value.kind {
            FunctionExprKind::ArrayLiteral { elems } => {
                self.emit_array_literal_into(value, elems, ptr)?;
                Ok(true)
            }
            FunctionExprKind::StructLiteral { fields, .. } => {
                self.emit_struct_literal_into(value, fields, ptr)?;
                Ok(true)
            }
            FunctionExprKind::UnionLiteral { field, .. } => {
                self.emit_union_literal_into(value, field, ptr)?;
                Ok(true)
            }
            FunctionExprKind::UnionStorageLiteral { bytes, relocations } => {
                self.emit_union_storage_literal_into(value, bytes, relocations, ptr)?;
                Ok(true)
            }
            FunctionExprKind::Null => {
                self.emit_tagged_union_into(
                    value,
                    FunctionOptionalTag::Null.discriminant(),
                    None,
                    ptr,
                )?;
                Ok(true)
            }
            FunctionExprKind::OptionalSome { expr } => {
                self.emit_tagged_union_into(
                    value,
                    FunctionOptionalTag::Some.discriminant(),
                    Some(expr),
                    ptr,
                )?;
                Ok(true)
            }
            FunctionExprKind::ErrorOk { expr } => {
                self.emit_tagged_union_into(
                    value,
                    FunctionErrorUnionTag::Ok.discriminant(),
                    Some(expr),
                    ptr,
                )?;
                Ok(true)
            }
            FunctionExprKind::ErrorErr { expr } => {
                self.emit_tagged_union_into(
                    value,
                    FunctionErrorUnionTag::Err.discriminant(),
                    Some(expr),
                    ptr,
                )?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn emit_aggregate_call_result_into(
        &mut self,
        ptr: nia_llvm::values::PointerValue<'ctx>,
        value: &FunctionExpr,
    ) -> Result<bool, Diagnostic> {
        let FunctionExprKind::Call { callee, args } = &value.kind else {
            return Ok(false);
        };
        if !matches!(
            self.module.classify_function_return(value.ty),
            crate::module_codegen::AbiReturn::IndirectOut(_)
        ) {
            return Ok(false);
        }
        if callee_is_extern(self, callee) {
            let result = self.emit_expr(value)?;
            self.builder
                .build_store(ptr, result)
                .map_err(|_| self.error(value.span, "failed to store aggregate call result"))?;
            return Ok(true);
        }
        let _ = self.emit_call_raw_with_out(value, callee, args, Some(ptr))?;
        Ok(true)
    }

    pub(super) fn emit_effect_expr(&mut self, expr: &FunctionExpr) -> Result<(), Diagnostic> {
        match &expr.kind {
            FunctionExprKind::Assign { place, op, rhs } => {
                if *op == nia_ast::AssignOp::Assign
                    && self.emit_direct_store_expr(expr.span, place, rhs)?
                {
                    return Ok(());
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
                if let crate::module_codegen::AbiReturn::IndirectOut(ty) =
                    self.module.classify_function_return(expr.ty)
                {
                    if callee_is_extern(self, callee) {
                        let _ = self.emit_call_raw(expr, callee, args)?;
                    } else {
                        let result_ty = self.module.llvm_basic_type(ty, expr.span)?;
                        let result_ptr = self
                            .builder
                            .build_alloca(result_ty, "discard.call.out")
                            .map_err(|_| {
                            self.error(expr.span, "failed to allocate discarded call result")
                        })?;
                        let _ =
                            self.emit_call_raw_with_out(expr, callee, args, Some(result_ptr))?;
                    }
                } else {
                    let _ = self.emit_call_raw(expr, callee, args)?;
                }
                Ok(())
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
            | FunctionExprKind::Null
            | FunctionExprKind::StaticArrayPointer { .. } => Ok(()),
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
                for _ in 0..count {
                    self.emit_effect_expr(value)?;
                }
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

fn is_aggregate_literal(value: &FunctionExpr) -> bool {
    matches!(
        value.kind,
        FunctionExprKind::ArrayLiteral { .. }
            | FunctionExprKind::StructLiteral { .. }
            | FunctionExprKind::UnionLiteral { .. }
            | FunctionExprKind::UnionStorageLiteral { .. }
            | FunctionExprKind::Null
            | FunctionExprKind::OptionalSome { .. }
            | FunctionExprKind::ErrorOk { .. }
            | FunctionExprKind::ErrorErr { .. }
    )
}

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    fn is_direct_store_candidate(&self, value: &FunctionExpr) -> bool {
        is_aggregate_literal(value)
            || (matches!(value.kind, FunctionExprKind::Call { .. })
                && matches!(
                    self.module.classify_function_return(value.ty),
                    crate::module_codegen::AbiReturn::IndirectOut(_)
                ))
    }
}
