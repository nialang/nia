// SPDX-License-Identifier: GPL-3.0-or-later
//! Memory-intrinsic validation contracts.

use super::*;

impl BackendValidator<'_> {
    pub(super) fn validate_memory_intrinsic(
        &mut self,
        memory: &nia_function_ir::FunctionMemoryIntrinsic,
    ) {
        use nia_function_ir::{FunctionMemoryIntrinsicOp as Op, FunctionMemoryIntrinsicSource};

        self.current_subject = Some("memory intrinsic element");
        self.validate_runtime_type(memory.elem_ty, memory.span);
        self.current_subject = None;

        self.validate_expr(&memory.dest);
        match self.index.ty_kind(memory.dest.ty).cloned() {
            Some(TyKind::Slice { is_readonly, elem }) => {
                if is_readonly {
                    self.invalid_memory_intrinsic(memory.span, "destination slice is readonly");
                }
                if !self.same_type(elem, memory.elem_ty) {
                    self.invalid_memory_intrinsic(
                        memory.span,
                        "destination element type does not match its element metadata",
                    );
                }
            }
            _ => self.invalid_memory_intrinsic(memory.span, "destination is not a slice"),
        }

        match (&memory.op, &memory.source) {
            (Op::Copy | Op::Move, FunctionMemoryIntrinsicSource::Slice(source)) => {
                self.validate_expr(source);
                match self.index.ty_kind(source.ty).cloned() {
                    Some(TyKind::Slice { elem, .. }) => {
                        if !self.same_type(elem, memory.elem_ty) {
                            self.invalid_memory_intrinsic(
                                memory.span,
                                "source element type does not match its element metadata",
                            );
                        }
                    }
                    _ => self.invalid_memory_intrinsic(memory.span, "source is not a slice"),
                }
            }
            (Op::Set, FunctionMemoryIntrinsicSource::Byte(value)) => {
                self.validate_expr(value);
                if !matches!(
                    self.index.ty_kind(memory.elem_ty),
                    Some(TyKind::Primitive(PrimitiveTy::U8))
                ) {
                    self.invalid_memory_intrinsic(
                        memory.span,
                        "set operation element type is not u8",
                    );
                }
                if !matches!(
                    self.index.ty_kind(value.ty),
                    Some(TyKind::Primitive(PrimitiveTy::U8))
                ) {
                    self.invalid_memory_intrinsic(memory.span, "set source is not a u8 value");
                }
            }
            (Op::Copy | Op::Move, FunctionMemoryIntrinsicSource::Byte(value)) => {
                self.validate_expr(value);
                self.invalid_memory_intrinsic(
                    memory.span,
                    "copy or move operation requires a slice source",
                );
            }
            (Op::Set, FunctionMemoryIntrinsicSource::Slice(source)) => {
                self.validate_expr(source);
                self.invalid_memory_intrinsic(memory.span, "set operation requires a byte source");
            }
        }
    }

    fn invalid_memory_intrinsic(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR memory intrinsic has an invalid contract: {message}"),
        ));
    }
}
