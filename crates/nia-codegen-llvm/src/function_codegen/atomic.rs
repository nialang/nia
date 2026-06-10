// SPDX-License-Identifier: GPL-3.0-or-later
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    AtomicOrder, AtomicRmwOp, FunctionAtomic, FunctionExpr, FunctionOptionalTag,
};
use nia_ids::InternedTyId;
use nia_llvm::{
    AtomicOrdering, AtomicRMWBinOp,
    values::{BasicValueEnum, PointerValue},
};
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyKind};

use super::FunctionCodegen;

struct CmpxchgOperands<'a> {
    ty: InternedTyId,
    ptr: &'a FunctionExpr,
    expected: &'a FunctionExpr,
    desired: &'a FunctionExpr,
    success: AtomicOrder,
    failure: AtomicOrder,
    weak: bool,
}

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(super) fn emit_atomic_value(
        &mut self,
        expr: &FunctionExpr,
        atomic: &FunctionAtomic,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        match atomic {
            FunctionAtomic::Load { ty, ptr, order } => {
                self.emit_atomic_load(expr, *ty, ptr, *order)
            }
            FunctionAtomic::Rmw {
                ty,
                ptr,
                op,
                value,
                order,
            } => self.emit_atomic_rmw(expr, *ty, ptr, *op, value, *order),
            FunctionAtomic::Cmpxchg {
                ty,
                ptr,
                expected,
                desired,
                success,
                failure,
                weak,
            } => self.emit_cmpxchg(
                expr,
                CmpxchgOperands {
                    ty: *ty,
                    ptr,
                    expected,
                    desired,
                    success: *success,
                    failure: *failure,
                    weak: *weak,
                },
            ),
            FunctionAtomic::Store { .. } | FunctionAtomic::Fence { .. } => Err(self.error(
                expr.span,
                "atomic store and fence do not produce runtime values",
            )),
        }
    }

    pub(super) fn emit_atomic_effect(
        &mut self,
        expr: &FunctionExpr,
        atomic: &FunctionAtomic,
    ) -> Result<(), Diagnostic> {
        match atomic {
            FunctionAtomic::Store {
                ty,
                ptr,
                value,
                order,
            } => self.emit_atomic_store(expr, *ty, ptr, value, *order),
            FunctionAtomic::Fence { order } => self.emit_fence(expr.span, *order),
            _ => {
                let _ = self.emit_atomic_value(expr, atomic)?;
                Ok(())
            }
        }
    }

    fn emit_atomic_load(
        &mut self,
        expr: &FunctionExpr,
        ty: InternedTyId,
        ptr: &FunctionExpr,
        order: AtomicOrder,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        self.validate_atomic_ty(expr.span, ty)?;
        let ptr = self.emit_atomic_ptr(ptr)?;
        let llvm_ty = self.module.llvm_basic_type(ty, expr.span)?;
        let value = self
            .builder
            .build_load(llvm_ty, ptr, "atomic.load")
            .map_err(|_| self.error(expr.span, "failed to build atomic load"))?;
        let Some(inst) = value.as_instruction_value() else {
            return Err(self.error(expr.span, "atomic load did not produce an instruction"));
        };
        inst.set_atomic_ordering(llvm_atomic_order(order));
        Ok(value)
    }

    fn emit_atomic_store(
        &mut self,
        expr: &FunctionExpr,
        _ty: InternedTyId,
        ptr: &FunctionExpr,
        value: &FunctionExpr,
        order: AtomicOrder,
    ) -> Result<(), Diagnostic> {
        self.validate_atomic_ty(expr.span, _ty)?;
        let ptr = self.emit_atomic_ptr(ptr)?;
        let value = self.emit_expr(value)?;
        let inst = self
            .builder
            .build_store(ptr, value)
            .map_err(|_| self.error(expr.span, "failed to build atomic store"))?;
        inst.set_atomic_ordering(llvm_atomic_order(order));
        Ok(())
    }

    fn emit_atomic_rmw(
        &mut self,
        expr: &FunctionExpr,
        _ty: InternedTyId,
        ptr: &FunctionExpr,
        op: AtomicRmwOp,
        value: &FunctionExpr,
        order: AtomicOrder,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        self.validate_atomic_ty(expr.span, _ty)?;
        let ptr = self.emit_atomic_ptr(ptr)?;
        let value = self.emit_expr(value)?;
        self.builder
            .build_atomicrmw(llvm_atomic_rmw_op(op), ptr, value, llvm_atomic_order(order))
            .map_err(|_| self.error(expr.span, "failed to build atomic read-modify-write"))
    }

    fn emit_cmpxchg(
        &mut self,
        expr: &FunctionExpr,
        operands: CmpxchgOperands<'_>,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        self.validate_atomic_ty(expr.span, operands.ty)?;
        let ptr = self.emit_atomic_ptr(operands.ptr)?;
        let expected = self.emit_expr(operands.expected)?;
        let desired = self.emit_expr(operands.desired)?;
        let result = self
            .builder
            .build_cmpxchg(
                ptr,
                expected,
                desired,
                llvm_atomic_order(operands.success),
                llvm_atomic_order(operands.failure),
                operands.weak,
            )
            .map_err(|_| self.error(expr.span, "failed to build cmpxchg"))?;
        let old = self
            .builder
            .build_extract_value(result, 0, "cmpxchg.old")
            .map_err(|_| self.error(expr.span, "failed to extract cmpxchg old value"))?;
        let ok = self
            .builder
            .build_extract_value(result, 1, "cmpxchg.ok")
            .map_err(|_| self.error(expr.span, "failed to extract cmpxchg status"))?
            .into_int_value()?;

        let optional_ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
        let out = self
            .builder
            .build_alloca(optional_ty, "cmpxchg.optional")
            .map_err(|_| self.error(expr.span, "failed to allocate cmpxchg optional"))?;
        let success_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "cmpxchg.success")?;
        let failure_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "cmpxchg.failure")?;
        let done_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "cmpxchg.done")?;
        self.builder
            .build_conditional_branch(ok, success_block, failure_block)
            .map_err(|_| self.error(expr.span, "failed to branch on cmpxchg status"))?;

        self.builder.position_at_end(success_block);
        self.store_optional_tag(
            expr.span,
            expr.ty,
            out,
            FunctionOptionalTag::Null.discriminant(),
        )?;
        self.builder
            .build_unconditional_branch(done_block)
            .map_err(|_| self.error(expr.span, "failed to finish cmpxchg success"))?;

        self.builder.position_at_end(failure_block);
        self.store_optional_tag(
            expr.span,
            expr.ty,
            out,
            FunctionOptionalTag::Some.discriminant(),
        )?;
        let payload_ptr = self
            .builder
            .build_struct_gep(optional_ty, out, 1, "cmpxchg.payload")
            .map_err(|_| self.error(expr.span, "failed to build cmpxchg payload address"))?;
        self.builder
            .build_store(payload_ptr, old)
            .map_err(|_| self.error(expr.span, "failed to store cmpxchg old value"))?;
        self.builder
            .build_unconditional_branch(done_block)
            .map_err(|_| self.error(expr.span, "failed to finish cmpxchg failure"))?;

        self.builder.position_at_end(done_block);
        self.builder
            .build_load(optional_ty, out, "cmpxchg.result")
            .map_err(|_| self.error(expr.span, "failed to load cmpxchg optional"))
    }

    fn emit_fence(&mut self, span: Span, order: AtomicOrder) -> Result<(), Diagnostic> {
        self.builder
            .build_fence(llvm_atomic_order(order), 0, "")
            .map_err(|_| self.error(span, "failed to build atomic fence"))?;
        Ok(())
    }

    fn emit_atomic_ptr(&mut self, ptr: &FunctionExpr) -> Result<PointerValue<'ctx>, Diagnostic> {
        self.emit_expr(ptr)?
            .into_pointer_value()
            .map_err(Into::into)
    }

    fn store_optional_tag(
        &mut self,
        span: Span,
        optional_ty: InternedTyId,
        ptr: PointerValue<'ctx>,
        tag: u8,
    ) -> Result<(), Diagnostic> {
        let optional_ty = self.module.llvm_basic_type(optional_ty, span)?;
        let tag_ptr = self
            .builder
            .build_struct_gep(optional_ty, ptr, 0, "optional.tag")
            .map_err(|_| self.error(span, "failed to build optional tag address"))?;
        let tag = self.module.context.i8_type().const_int(tag.into(), false);
        self.builder
            .build_store(tag_ptr, tag)
            .map_err(|_| self.error(span, "failed to store optional tag"))?;
        Ok(())
    }

    fn validate_atomic_ty(&self, span: Span, ty: InternedTyId) -> Result<(), Diagnostic> {
        let layout = self.module.layout_of(ty);
        let pointer_size = self.module.source.layouts.target.pointer_size;
        let valid = match self.module.ty_kind(ty) {
            Some(TyKind::Primitive(primitive)) => matches!(
                primitive,
                PrimitiveTy::Bool
                    | PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::Isize
                    | PrimitiveTy::U8
                    | PrimitiveTy::U16
                    | PrimitiveTy::U32
                    | PrimitiveTy::U64
                    | PrimitiveTy::Usize
                    | PrimitiveTy::Char
            ),
            Some(TyKind::Pointer { .. }) => true,
            Some(TyKind::Nominal { def_id, .. }) => self.module.program.enums.contains_key(def_id),
            _ => false,
        };
        if valid && layout.is_some_and(|layout| layout.size <= pointer_size) {
            Ok(())
        } else {
            Err(self.error(span, "unsupported atomic value type"))
        }
    }
}

fn llvm_atomic_order(order: AtomicOrder) -> AtomicOrdering {
    match order {
        AtomicOrder::Unordered => AtomicOrdering::Unordered,
        AtomicOrder::Monotonic => AtomicOrdering::Monotonic,
        AtomicOrder::Acquire => AtomicOrdering::Acquire,
        AtomicOrder::Release => AtomicOrdering::Release,
        AtomicOrder::AcqRel => AtomicOrdering::AcquireRelease,
        AtomicOrder::SeqCst => AtomicOrdering::SequentiallyConsistent,
    }
}

fn llvm_atomic_rmw_op(op: AtomicRmwOp) -> AtomicRMWBinOp {
    match op {
        AtomicRmwOp::Xchg => AtomicRMWBinOp::Xchg,
        AtomicRmwOp::Add => AtomicRMWBinOp::Add,
        AtomicRmwOp::Sub => AtomicRMWBinOp::Sub,
        AtomicRmwOp::And => AtomicRMWBinOp::And,
        AtomicRmwOp::Nand => AtomicRMWBinOp::Nand,
        AtomicRmwOp::Or => AtomicRMWBinOp::Or,
        AtomicRmwOp::Xor => AtomicRMWBinOp::Xor,
        AtomicRmwOp::Max => AtomicRMWBinOp::Max,
        AtomicRmwOp::Min => AtomicRMWBinOp::Min,
        AtomicRmwOp::UMax => AtomicRMWBinOp::UMax,
        AtomicRmwOp::UMin => AtomicRMWBinOp::UMin,
    }
}
