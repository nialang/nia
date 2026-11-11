// SPDX-License-Identifier: GPL-3.0-or-later
//! Where expression values live.
//!
//! [`ValueRepr`] divides types in two. Vectors and ABI-direct types are immediate;
//! aggregates are memory values. Wide vectors still use addresses at calls,
//! so their ABI stays independent of function CPU features. An immediate
//! value is an SSA value, which [`FunctionCodegen::emit_expr`] returns. A
//! memory value exists only in storage: [`FunctionCodegen::emit_into`] writes
//! one into fresh storage, [`FunctionCodegen::emit_memory_operand`] gives the
//! address of one to a consumer that reads it, and copies between storage use
//! `llvm.memcpy`.
//!
//! Fresh storage is storage that the expression being written cannot read: a
//! new binding, a temporary, a part of either, or the function's own result.
//! Values are built straight into it. An assignment target is not fresh, as
//! the right-hand side may read it (`s = f(&s)`, `p = P { a: p.b, b: p.a }`);
//! [`FunctionCodegen::emit_memory_assign`] handles that case.
//!
//! Temporaries are allocated in the entry block, so that one in a loop does
//! not grow the frame on every iteration.

use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    FunctionErrorUnionTag, FunctionExpr, FunctionExprKind, FunctionOptionalTag, FunctionPlace,
    FunctionPlaceBase, FunctionPlaceElem,
};
use nia_ids::{InternedTyId, IntrinsicOp};
use nia_llvm::{
    types::BasicTypeEnum,
    values::{BasicValueEnum, PointerValue},
};
use nia_span::Span;
use nia_ty::TyKind;

use super::FunctionCodegen;
use crate::module_codegen::{ModuleCodegen, ValueRepr};

/// Storage that already holds a memory value.
struct Stored<'ctx> {
    ptr: PointerValue<'ctx>,
    volatile: bool,
}

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(super) fn value_repr(&self, ty: InternedTyId) -> ValueRepr {
        self.module.value_repr(ty)
    }

    pub(super) fn is_memory_value(&self, ty: InternedTyId) -> bool {
        self.value_repr(ty) == ValueRepr::Memory
    }

    /// A stack slot in the entry block.
    pub(super) fn entry_alloca(
        &self,
        ty: BasicTypeEnum<'ctx>,
        align: u64,
        name: &str,
        span: Span,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        let entry = self
            .llvm_function
            .get_first_basic_block()
            .ok_or_else(|| self.error(span, "function has no physical entry block"))?;
        let builder = self
            .module
            .context
            .create_builder()
            .map_err(ModuleCodegen::diagnostic_from_llvm_error)?;
        match entry.get_first_instruction() {
            Some(first) => builder.position_before(&first),
            None => builder.position_at_end(entry),
        }
        let slot = builder
            .build_alloca(ty, name)
            .map_err(|_| self.error(span, "failed to allocate stack storage"))?;
        let align = u32::try_from(align)
            .map_err(|_| self.error(span, "stack storage alignment is too large"))?;
        BasicValueEnum::from(slot)
            .as_instruction_value()
            .ok_or_else(|| self.error(span, "stack storage is not an instruction"))?
            .set_alignment(align)
            .map_err(|_| self.error(span, "failed to align stack storage"))?;
        Ok(slot)
    }

    /// Entry-block storage for one value of `ty`.
    pub(super) fn temporary(
        &self,
        ty: InternedTyId,
        name: &str,
        span: Span,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        let llvm_ty = self.module.llvm_basic_type(ty, span)?;
        let align = self.module.layout_of(ty).map_or(1, |layout| layout.align);
        self.entry_alloca(llvm_ty, align, name, span)
    }

    /// Copies a value of `ty` from `src` to `dest` with `llvm.memcpy`, which
    /// is volatile when either side is.
    pub(super) fn copy_value(
        &self,
        dest: PointerValue<'ctx>,
        dest_volatile: bool,
        src: PointerValue<'ctx>,
        src_volatile: bool,
        ty: InternedTyId,
        span: Span,
    ) -> Result<(), Diagnostic> {
        let layout = self
            .module
            .layout_of(ty)
            .ok_or_else(|| self.error(span, "copied value has no layout"))?;
        // A returned local that lives in the result slot is its own copy.
        if layout.size == 0 || dest == src {
            return Ok(());
        }
        let align = u32::try_from(layout.align)
            .map_err(|_| self.error(span, "copied value alignment is too large"))?;
        let size = self
            .module
            .usize_llvm_type(span)?
            .const_int(layout.size, false)?;
        let copy = if dest_volatile || src_volatile {
            self.builder
                .build_volatile_memcpy(dest, align, src, align, size)
        } else {
            self.builder.build_memcpy(dest, align, src, align, size)
        };
        copy.map_err(|_| self.error(span, "failed to copy value"))?;
        Ok(())
    }

    /// Copies the value of `ty` stored at `src` into fresh storage `dest`,
    /// whatever its representation.
    pub(super) fn transfer_value(
        &self,
        dest: PointerValue<'ctx>,
        src: PointerValue<'ctx>,
        ty: InternedTyId,
        span: Span,
    ) -> Result<(), Diagnostic> {
        match self.value_repr(ty) {
            ValueRepr::ZeroSized => Ok(()),
            ValueRepr::Immediate => {
                let llvm_ty = self.module.llvm_basic_type(ty, span)?;
                let value = self
                    .builder
                    .build_load(llvm_ty, src, "transfer")
                    .map_err(|_| self.error(span, "failed to load value"))?;
                self.builder
                    .build_store(dest, value)
                    .map_err(|_| self.error(span, "failed to store value"))?;
                Ok(())
            }
            ValueRepr::Memory => self.copy_value(dest, false, src, false, ty, span),
        }
    }

    /// Writes the value of `expr` into fresh storage `dest`.
    pub(super) fn emit_into(
        &mut self,
        expr: &FunctionExpr,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Diagnostic> {
        match self.value_repr(expr.ty) {
            ValueRepr::ZeroSized => self.emit_effect_expr(expr),
            ValueRepr::Immediate => {
                let value = self.emit_expr(expr)?;
                self.builder
                    .build_store(dest, value)
                    .map_err(|_| self.error(expr.span, "failed to store value"))?;
                Ok(())
            }
            ValueRepr::Memory => match self.stored_value(expr)? {
                Some(stored) => {
                    self.copy_value(dest, false, stored.ptr, stored.volatile, expr.ty, expr.span)
                }
                None => self.build_memory_value(expr, dest),
            },
        }
    }

    /// Assigns memory-represented `rhs` to `place`.
    ///
    /// The right-hand side is evaluated first and may read the place, so a
    /// value it builds goes through a temporary. A value it names in another
    /// place is copied directly, unless evaluating the place could write
    /// that storage, as `a[f()] = s` can when `f` changes `s`.
    pub(super) fn emit_memory_assign(
        &mut self,
        span: Span,
        place: &FunctionPlace,
        rhs: &FunctionExpr,
    ) -> Result<(), Diagnostic> {
        let (src, src_volatile) = match self.stored_value(rhs)? {
            Some(stored) if self.place_is_pure(place) => (stored.ptr, stored.volatile),
            Some(stored) => {
                let snapshot = self.temporary(rhs.ty, "assign.value", rhs.span)?;
                self.copy_value(snapshot, false, stored.ptr, stored.volatile, rhs.ty, span)?;
                (snapshot, false)
            }
            None => {
                let value = self.temporary(rhs.ty, "assign.value", rhs.span)?;
                self.build_memory_value(rhs, value)?;
                (value, false)
            }
        };
        if self.current_block_has_terminator() {
            return Ok(());
        }
        let dest = self.emit_typed_place_addr(place)?;
        let dest_volatile = self.place_access_is_volatile(place);
        self.copy_value(dest, dest_volatile, src, src_volatile, rhs.ty, span)
    }

    /// The address of storage holding the value of `expr`, which lives in
    /// memory or has no size. The consumer must read it before anything
    /// else is written: it may be the storage of a place.
    pub(super) fn emit_memory_operand(
        &mut self,
        expr: &FunctionExpr,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        match self.value_repr(expr.ty) {
            ValueRepr::Memory => {}
            ValueRepr::Immediate => {
                return Err(self.error(expr.span, "memory operand has an immediate type"));
            }
            ValueRepr::ZeroSized => {
                if let Some(stored) = self.stored_value(expr)? {
                    return Ok(stored.ptr);
                }
                self.emit_effect_expr(expr)?;
                let align = self
                    .module
                    .layout_of(expr.ty)
                    .map_or(1, |layout| layout.align);
                let byte = self.module.context.i8_type().into();
                return self.entry_alloca(byte, align, "zst.value", expr.span);
            }
        }
        match self.stored_value(expr)? {
            Some(Stored {
                ptr,
                volatile: false,
            }) => Ok(ptr),
            Some(Stored {
                ptr,
                volatile: true,
            }) => {
                let temp = self.temporary(expr.ty, "volatile.copy", expr.span)?;
                self.copy_value(temp, false, ptr, true, expr.ty, expr.span)?;
                Ok(temp)
            }
            None => {
                let temp = self.temporary(expr.ty, "value.tmp", expr.span)?;
                self.build_memory_value(expr, temp)?;
                Ok(temp)
            }
        }
    }

    /// The storage of a memory value that already exists: a place, a part of
    /// another memory value, or a compiler-owned constant.
    fn stored_value(&mut self, expr: &FunctionExpr) -> Result<Option<Stored<'ctx>>, Diagnostic> {
        let stored = |ptr| {
            Some(Stored {
                ptr,
                volatile: false,
            })
        };
        match &expr.kind {
            FunctionExprKind::Local(_)
            | FunctionExprKind::Global(_)
            | FunctionExprKind::GlobalInstance { .. }
            | FunctionExprKind::Field { .. }
            | FunctionExprKind::Index { .. } => Ok(stored(self.emit_addr_of(expr)?)),
            FunctionExprKind::Unary {
                op: nia_ast::UnaryOp::Deref,
                expr: pointer,
            } => {
                let volatile = matches!(
                    self.module.ty_kind(pointer.ty),
                    Some(TyKind::VolatilePointer { .. })
                );
                let ptr = self.emit_expr(pointer)?.into_pointer_value()?;
                Ok(Some(Stored { ptr, volatile }))
            }
            FunctionExprKind::TupleField { value, index } => Ok(stored(
                self.emit_tuple_field_addr(expr.span, value, *index)?,
            )),
            FunctionExprKind::TaggedUnionPayload { expr: tagged } => {
                let tagged_ptr = self.emit_memory_operand(tagged)?;
                Ok(stored(self.tagged_union_payload_addr(
                    expr.span, tagged.ty, tagged_ptr,
                )?))
            }
            FunctionExprKind::EnumPayloadField {
                value,
                variant,
                field,
            } => Ok(stored(
                self.emit_enum_payload_field_addr(expr, value, *variant, *field)?,
            )),
            FunctionExprKind::CallerLocation(location) => Ok(stored(match self.caller_location {
                Some(pointer) => pointer,
                None => self
                    .module
                    .materialize_source_location(location, expr.span)?,
            })),
            _ => Ok(None),
        }
    }

    /// Builds the value of memory-represented `expr`, which is not already
    /// stored anywhere, in fresh storage `ptr`.
    fn build_memory_value(
        &mut self,
        expr: &FunctionExpr,
        ptr: PointerValue<'ctx>,
    ) -> Result<(), Diagnostic> {
        match &expr.kind {
            FunctionExprKind::ArrayLiteral { elems } => {
                self.emit_array_literal_into(expr, elems, ptr)
            }
            FunctionExprKind::Tuple(elems) => self.emit_tuple_literal_into(expr, elems, ptr),
            FunctionExprKind::StructLiteral { fields, .. } => {
                self.emit_struct_literal_into(expr, fields, ptr)
            }
            FunctionExprKind::UnionLiteral { field, .. } => {
                self.emit_union_literal_into(field, ptr)
            }
            FunctionExprKind::UnionStorageLiteral { bytes, relocations } => {
                self.emit_union_storage_literal_into(expr, bytes, relocations, ptr)
            }
            FunctionExprKind::String(_) | FunctionExprKind::ByteString(_) => {
                self.emit_constant_into(expr, ptr)
            }
            FunctionExprKind::Null => self.emit_tagged_union_into(
                expr,
                FunctionOptionalTag::Null.discriminant(),
                None,
                ptr,
            ),
            FunctionExprKind::OptionalSome { expr: payload } => self.emit_tagged_union_into(
                expr,
                FunctionOptionalTag::Some.discriminant(),
                Some(payload),
                ptr,
            ),
            FunctionExprKind::ErrorOk { expr: payload } => self.emit_tagged_union_into(
                expr,
                FunctionErrorUnionTag::Ok.discriminant(),
                Some(payload),
                ptr,
            ),
            FunctionExprKind::ErrorErr { expr: payload } => self.emit_tagged_union_into(
                expr,
                FunctionErrorUnionTag::Err.discriminant(),
                Some(payload),
                ptr,
            ),
            FunctionExprKind::EnumVariant { variant, fields } => {
                self.emit_enum_variant_into(expr, *variant, fields, ptr)
            }
            FunctionExprKind::Call { callee, args } => self.emit_call_into(expr, callee, args, ptr),
            FunctionExprKind::Atomic(atomic) => self.emit_atomic_into(expr, atomic, ptr),
            FunctionExprKind::CharFromU32 { value } => {
                self.emit_char_from_u32_into(expr, value, ptr)
            }
            FunctionExprKind::Intrinsic {
                op: IntrinsicOp::LoadUnaligned,
                args,
            } => {
                let [source] = args.as_slice() else {
                    return Err(self.error(expr.span, "unaligned load takes one pointer operand"));
                };
                let source = self.emit_expr(source)?.into_pointer_value()?;
                let layout = self
                    .module
                    .layout_of(expr.ty)
                    .ok_or_else(|| self.error(expr.span, "unaligned load has no layout"))?;
                let size = self
                    .module
                    .usize_llvm_type(expr.span)?
                    .const_int(layout.size, false)?;
                let align = u32::try_from(layout.align)
                    .map_err(|_| self.error(expr.span, "value alignment is too large"))?;
                self.builder
                    .build_memcpy(ptr, align, source, 1, size)
                    .map_err(|_| self.error(expr.span, "failed to build unaligned load"))?;
                Ok(())
            }
            _ => Err(self.error(expr.span, "expression has no memory-represented value")),
        }
    }

    /// Copies the constant value of a literal from a read-only image.
    fn emit_constant_into(
        &mut self,
        expr: &FunctionExpr,
        ptr: PointerValue<'ctx>,
    ) -> Result<(), Diagnostic> {
        let value = match &expr.kind {
            FunctionExprKind::String(scalars) => {
                self.emit_string_literal(expr.ty, expr.span, scalars)?
            }
            FunctionExprKind::ByteString(bytes) => {
                self.emit_byte_string_literal(expr.ty, expr.span, bytes)?
            }
            _ => return Err(self.error(expr.span, "expression is not a constant literal")),
        };
        let align = self
            .module
            .layout_of(expr.ty)
            .map_or(1, |layout| layout.align);
        let image = self.module.constant_image(value, align, expr.span)?;
        self.copy_value(ptr, false, image, false, expr.ty, expr.span)
    }

    /// Whether computing the address of `place` cannot write memory.
    fn place_is_pure(&self, place: &FunctionPlace) -> bool {
        let base = match &place.base {
            FunctionPlaceBase::Deref(pointer) => expr_is_pure(pointer),
            FunctionPlaceBase::Local(_)
            | FunctionPlaceBase::Global(_)
            | FunctionPlaceBase::GlobalInstance { .. }
            | FunctionPlaceBase::Error => true,
        };
        base && place.elems.iter().all(|elem| match elem {
            FunctionPlaceElem::Index(index) => expr_is_pure(index),
            FunctionPlaceElem::Field(_)
            | FunctionPlaceElem::TupleField(_)
            | FunctionPlaceElem::Error => true,
        })
    }
}

/// Whether evaluating `expr` cannot write memory. Conservative: only reads,
/// constants, and arithmetic qualify.
fn expr_is_pure(expr: &FunctionExpr) -> bool {
    match &expr.kind {
        FunctionExprKind::Local(_)
        | FunctionExprKind::Global(_)
        | FunctionExprKind::GlobalInstance { .. }
        | FunctionExprKind::Integer(_)
        | FunctionExprKind::Float(_)
        | FunctionExprKind::Bool(_)
        | FunctionExprKind::Char(_)
        | FunctionExprKind::ByteChar(_)
        | FunctionExprKind::BuiltinValue(_) => true,
        FunctionExprKind::Field { lhs, .. } => expr_is_pure(lhs),
        FunctionExprKind::Index { lhs, index } => expr_is_pure(lhs) && expr_is_pure(index),
        FunctionExprKind::TupleField { value, .. } => expr_is_pure(value),
        FunctionExprKind::Unary { expr, .. } | FunctionExprKind::Cast { expr, .. } => {
            expr_is_pure(expr)
        }
        FunctionExprKind::Binary { lhs, rhs, .. } => expr_is_pure(lhs) && expr_is_pure(rhs),
        _ => false,
    }
}
