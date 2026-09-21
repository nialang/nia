// SPDX-License-Identifier: GPL-3.0-or-later
//! Validation contracts for LLVM atomic operations.

use super::*;
use crate::backend_validate::function_ir::AtomicOrderContext;
use nia_function_ir::{AtomicOrder, AtomicRmwOp};

impl BackendValidator<'_> {
    pub(super) fn validate_atomic(
        &mut self,
        atomic: &nia_function_ir::FunctionAtomic,
        result_ty: nia_ids::InternedTyId,
        span: Span,
    ) {
        // LLVM assigns different legal ordering sets to each atomic opcode and
        // encodes cmpxchg's old value in an aggregate. Validate the complete
        // typed contract here so malformed producer IR cannot reach builder
        // calls whose failure behavior varies across LLVM versions.
        match atomic {
            nia_function_ir::FunctionAtomic::Load { ty, ptr, order } => {
                self.validate_atomic_value_type(*ty, span);
                self.validate_atomic_pointer(ptr.ty, *ty, false, span);
                self.validate_atomic_order(*order, AtomicOrderContext::Load, span);
                self.validate_atomic_result_type(result_ty, *ty, span, "load");
                self.validate_expr(ptr);
            }
            nia_function_ir::FunctionAtomic::Store {
                ty,
                ptr,
                value,
                order,
            } => {
                self.validate_atomic_value_type(*ty, span);
                self.validate_atomic_pointer(ptr.ty, *ty, true, span);
                self.validate_atomic_result_type(value.ty, *ty, span, "store value");
                self.validate_atomic_order(*order, AtomicOrderContext::Store, span);
                self.validate_expr(ptr);
                self.validate_expr(value);
            }
            nia_function_ir::FunctionAtomic::Rmw {
                ty,
                ptr,
                op,
                value,
                order,
            } => {
                self.validate_atomic_value_type(*ty, span);
                self.validate_atomic_pointer(ptr.ty, *ty, true, span);
                self.validate_atomic_result_type(value.ty, *ty, span, "RMW value");
                self.validate_atomic_result_type(result_ty, *ty, span, "RMW result");
                self.validate_atomic_rmw_type(*ty, *op, span);
                self.validate_atomic_order(*order, AtomicOrderContext::Rmw, span);
                self.validate_expr(ptr);
                self.validate_expr(value);
            }
            nia_function_ir::FunctionAtomic::Cmpxchg {
                ty,
                ptr,
                expected,
                desired,
                success,
                failure,
                ..
            } => {
                self.validate_atomic_value_type(*ty, span);
                self.validate_atomic_pointer(ptr.ty, *ty, true, span);
                self.validate_atomic_result_type(expected.ty, *ty, span, "cmpxchg expected value");
                self.validate_atomic_result_type(desired.ty, *ty, span, "cmpxchg desired value");
                match self.index.ty_kind(result_ty).cloned() {
                    Some(TyKind::Optional { elem }) if self.same_type(elem, *ty) => {}
                    _ => self.invalid_atomic_contract(
                        span,
                        "cmpxchg result must be optional over its atomic value type",
                    ),
                }
                self.validate_atomic_order(*success, AtomicOrderContext::CmpxchgSuccess, span);
                self.validate_atomic_order(*failure, AtomicOrderContext::CmpxchgFailure, span);
                if !cmpxchg_failure_order_allowed(*success, *failure) {
                    self.invalid_atomic_contract(
                        span,
                        "cmpxchg failure ordering is stronger than or incomparable with its success ordering",
                    );
                }
                self.validate_expr(ptr);
                self.validate_expr(expected);
                self.validate_expr(desired);
            }
            nia_function_ir::FunctionAtomic::Fence { order } => {
                self.validate_atomic_order(*order, AtomicOrderContext::Fence, span);
            }
        }
    }

    fn validate_atomic_value_type(&mut self, ty: nia_ids::InternedTyId, span: Span) {
        self.validate_runtime_type(ty, span);
        let valid_kind = match self.index.ty_kind(ty) {
            Some(TyKind::Primitive(
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
                | PrimitiveTy::Char,
            ))
            | Some(TyKind::Pointer { .. }) => true,
            Some(TyKind::Nominal { def_id, .. }) => self.index.has_enum(*def_id),
            _ => false,
        };
        let fits_target = self
            .layout_of(ty)
            .is_some_and(|layout| layout.size <= self.target.pointer_size);
        if !valid_kind || !fits_target {
            self.invalid_atomic_contract(
                span,
                "value type is not a pointer-width bool, integer, enum, or pointer",
            );
        }
    }

    fn validate_atomic_pointer(
        &mut self,
        ptr_ty: nia_ids::InternedTyId,
        value_ty: nia_ids::InternedTyId,
        requires_write: bool,
        span: Span,
    ) {
        match self.index.ty_kind(ptr_ty).cloned() {
            Some(TyKind::Pointer { is_readonly, elem }) => {
                if !self.same_type(elem, value_ty) {
                    self.invalid_atomic_contract(
                        span,
                        "pointer pointee does not match the atomic value type",
                    );
                }
                if requires_write && is_readonly {
                    self.invalid_atomic_contract(span, "mutating operation has a readonly pointer");
                }
            }
            _ => self.invalid_atomic_contract(span, "operand is not a pointer"),
        }
    }

    fn validate_atomic_result_type(
        &mut self,
        actual_ty: nia_ids::InternedTyId,
        expected_ty: nia_ids::InternedTyId,
        span: Span,
        subject: &'static str,
    ) {
        if !self.same_type(actual_ty, expected_ty) {
            self.invalid_atomic_contract(
                span,
                format!("{subject} type does not match the atomic value type"),
            );
        }
    }

    fn validate_atomic_rmw_type(&mut self, ty: nia_ids::InternedTyId, op: AtomicRmwOp, span: Span) {
        if op == AtomicRmwOp::Xchg {
            return;
        }
        let integer_like = match self.index.ty_kind(ty) {
            Some(TyKind::Primitive(
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
                | PrimitiveTy::Char,
            )) => true,
            Some(TyKind::Nominal { def_id, .. }) => self.index.has_enum(*def_id),
            _ => false,
        };
        if !integer_like {
            self.invalid_atomic_contract(
                span,
                "non-exchange RMW operation requires an integer-like value type",
            );
        }
    }

    fn validate_atomic_order(
        &mut self,
        order: AtomicOrder,
        context: AtomicOrderContext,
        span: Span,
    ) {
        if !context.allows(order) {
            self.invalid_atomic_contract(span, "ordering is invalid for the operation");
        }
    }

    fn invalid_atomic_contract(&mut self, span: Span, message: impl std::fmt::Display) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR atomic operation has an invalid contract: {message}"),
        ));
    }
}

fn cmpxchg_failure_order_allowed(success: AtomicOrder, failure: AtomicOrder) -> bool {
    match success {
        AtomicOrder::Monotonic => failure == AtomicOrder::Monotonic,
        AtomicOrder::Acquire => matches!(failure, AtomicOrder::Monotonic | AtomicOrder::Acquire),
        AtomicOrder::Release => failure == AtomicOrder::Monotonic,
        AtomicOrder::AcqRel => matches!(failure, AtomicOrder::Monotonic | AtomicOrder::Acquire),
        AtomicOrder::SeqCst => matches!(
            failure,
            AtomicOrder::Monotonic | AtomicOrder::Acquire | AtomicOrder::SeqCst
        ),
        AtomicOrder::Unordered => false,
    }
}
