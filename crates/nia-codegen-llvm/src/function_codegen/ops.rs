// SPDX-License-Identifier: GPL-3.0-or-later
use crate::literals::assign_to_binary_op;
use nia_ast::{AssignOp, BinaryOp, UnaryOp};
use nia_diagnostic::Diagnostic;
use nia_function_ir::{FunctionExpr, FunctionExprKind};
use nia_ids::InternedTyId;
use nia_llvm::{FloatPredicate, IntPredicate, values::BasicValueEnum};
use nia_span::Span;

use super::FunctionCodegen;

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(super) fn emit_unary(
        &mut self,
        span: Span,
        ty: InternedTyId,
        op: UnaryOp,
        inner: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        match op {
            UnaryOp::Ref | UnaryOp::RefReadOnly
                if matches!(
                    inner.kind,
                    FunctionExprKind::Function(_) | FunctionExprKind::FunctionInstance { .. }
                ) =>
            {
                self.emit_function_pointer(span, inner)
            }
            UnaryOp::Ref | UnaryOp::RefReadOnly => {
                Err(self.error(span, "address-of place must be lowered to function IR"))
            }
            UnaryOp::Deref => {
                let ptr = self.emit_expr(inner)?.into_pointer_value()?;
                let ty = self.module.llvm_basic_type(ty, span)?;
                let is_volatile = matches!(
                    self.module.ty_kind(inner.ty),
                    Some(nia_ty::TyKind::VolatilePointer { .. })
                );
                self.build_place_load(ty, ptr, "deref", is_volatile)
                    .map_err(|_| self.error(span, "failed to load dereference"))
            }
            UnaryOp::Neg => {
                let value = self.emit_expr(inner)?;
                if self.is_float(ty) {
                    self.builder
                        .build_basic_float_neg(value, "negtmp")
                        .map_err(|_| self.error(span, "failed to build float negation"))
                } else {
                    self.builder
                        .build_basic_neg(value, "negtmp")
                        .map_err(|_| self.error(span, "failed to build integer negation"))
                }
            }
            UnaryOp::Not => {
                let value = self.emit_expr(inner)?;
                self.builder
                    .build_basic_not(value, "nottmp")
                    .map_err(|_| self.error(span, "failed to build not"))
            }
            UnaryOp::BitNot => {
                let value = self.emit_expr(inner)?;
                self.builder
                    .build_basic_not(value, "bitnottmp")
                    .map_err(|_| self.error(span, "failed to build bitwise not"))
            }
        }
    }

    pub(super) fn emit_cast(
        &mut self,
        span: Span,
        source_ty: InternedTyId,
        target_ty: InternedTyId,
        value: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        if self.same_llvm_type(source_ty, target_ty, span)? {
            return Ok(value);
        }
        let target = self.module.llvm_basic_type(target_ty, span)?;
        if self.is_pointer_like(source_ty) && self.is_pointer_like(target_ty) {
            return self
                .builder
                .build_pointer_cast(
                    value.into_pointer_value()?,
                    target.into_pointer_type()?,
                    "casttmp",
                )
                .map(Into::into)
                .map_err(|_| self.error(span, "failed to build pointer cast"));
        }
        if self.is_pointer_like(source_ty) && self.is_pointer_integer(target_ty) {
            return self
                .builder
                .build_ptr_to_int(
                    value.into_pointer_value()?,
                    target.into_int_type()?,
                    "casttmp",
                )
                .map(Into::into)
                .map_err(|_| self.error(span, "failed to build pointer-to-int cast"));
        }
        if self.is_pointer_integer(source_ty) && self.is_pointer_like(target_ty) {
            return self
                .builder
                .build_int_to_ptr(
                    value.into_int_value()?,
                    target.into_pointer_type()?,
                    "casttmp",
                )
                .map(Into::into)
                .map_err(|_| self.error(span, "failed to build int-to-pointer cast"));
        }
        if self.is_float(source_ty) && self.is_float(target_ty) {
            return self
                .builder
                .build_float_cast(
                    value.into_float_value()?,
                    target.into_float_type()?,
                    "casttmp",
                )
                .map(Into::into)
                .map_err(|_| self.error(span, "failed to build float cast"));
        }
        if self.is_float(source_ty) && self.is_integer_like(target_ty) {
            let target = target.into_int_type()?;
            let result = if self.is_signed_integer(target_ty) {
                self.builder
                    .build_float_to_signed_int(value.into_float_value()?, target, "casttmp")
            } else {
                self.builder.build_float_to_unsigned_int(
                    value.into_float_value()?,
                    target,
                    "casttmp",
                )
            };
            return result
                .map(Into::into)
                .map_err(|_| self.error(span, "failed to build float-to-int cast"));
        }
        if self.is_integer_like(source_ty) && self.is_float(target_ty) {
            let target = target.into_float_type()?;
            let result = if self.is_signed_integer(source_ty) {
                self.builder
                    .build_signed_int_to_float(value.into_int_value()?, target, "casttmp")
            } else {
                self.builder
                    .build_unsigned_int_to_float(value.into_int_value()?, target, "casttmp")
            };
            return result
                .map(Into::into)
                .map_err(|_| self.error(span, "failed to build int-to-float cast"));
        }
        if self.is_integer_like(source_ty) && self.is_integer_like(target_ty) {
            let source_bits = self.integer_bits(source_ty, span)?;
            let target = target.into_int_type()?;
            let target_bits = target.bit_width();
            if source_bits == target_bits {
                return Ok(value);
            }
            let value = value.into_int_value()?;
            let result = if source_bits > target_bits {
                self.builder.build_int_truncate(value, target, "casttmp")
            } else if self.is_signed_integer(source_ty) {
                self.builder.build_int_s_extend(value, target, "casttmp")
            } else {
                self.builder.build_int_z_extend(value, target, "casttmp")
            };
            return result
                .map(Into::into)
                .map_err(|_| self.error(span, "failed to build integer cast"));
        }
        Err(self.error(
            span,
            "invalid cast reached LLVM codegen after type checking",
        ))
    }

    pub(super) fn emit_short_circuit(
        &mut self,
        span: Span,
        lhs: &FunctionExpr,
        op: BinaryOp,
        rhs: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let lhs_value = self.emit_expr(lhs)?.into_int_value()?;
        let rhs_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "logic.rhs")?;
        let merge_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "logic.end")?;
        let lhs_block = self
            .builder
            .get_insert_block()
            .ok_or_else(|| self.error(span, "missing current block for logical expression"))?;
        match op {
            BinaryOp::And => {
                self.builder
                    .build_conditional_branch(lhs_value, rhs_block, merge_block)
            }
            BinaryOp::Or => {
                self.builder
                    .build_conditional_branch(lhs_value, merge_block, rhs_block)
            }
            _ => {
                return Err(nia_ice::Ice::new(
                    "only logical operators reach short-circuit codegen",
                )
                .diagnostic());
            }
        }
        .map_err(|_| self.error(span, "failed to build logical branch"))?;

        self.builder.position_at_end(rhs_block);
        let rhs_value = self.emit_expr(rhs)?.into_int_value()?;
        let rhs_end = self
            .builder
            .get_insert_block()
            .ok_or_else(|| self.error(span, "missing rhs block for logical expression"))?;
        if !self.current_block_has_terminator() {
            self.builder
                .build_unconditional_branch(merge_block)
                .map_err(|_| self.error(span, "failed to build logical merge branch"))?;
        }

        self.builder.position_at_end(merge_block);
        let phi = self
            .builder
            .build_phi(self.module.context.bool_type(), "logictmp")
            .map_err(|_| self.error(span, "failed to build logical phi"))?;
        let short_value = self
            .module
            .context
            .bool_type()
            .const_int(u64::from(op == BinaryOp::Or), false);
        phi.add_incoming(&[(&short_value, lhs_block), (&rhs_value, rhs_end)]);
        Ok(phi.as_basic_value()?)
    }

    pub(super) fn emit_compound_assignment(
        &mut self,
        span: Span,
        operand_ty: InternedTyId,
        lhs: BasicValueEnum<'ctx>,
        op: AssignOp,
        rhs: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(op) = assign_to_binary_op(op) else {
            return Ok(rhs);
        };
        self.emit_binary(span, operand_ty, lhs, op, rhs)
    }

    pub(super) fn emit_binary(
        &mut self,
        span: Span,
        operand_ty: InternedTyId,
        lhs: BasicValueEnum<'ctx>,
        op: BinaryOp,
        rhs: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        if self.is_float(operand_ty) {
            return self.emit_float_binary(span, lhs, op, rhs);
        }
        let is_signed = self.is_signed_integer(operand_ty);
        let is_vector = matches!(
            self.module.ty_kind(operand_ty),
            Some(nia_ty::TyKind::Vector { .. })
        );
        let result = match op {
            BinaryOp::Add => self.builder.build_basic_int_add(lhs, rhs, "addtmp"),
            BinaryOp::Sub => self.builder.build_basic_int_sub(lhs, rhs, "subtmp"),
            BinaryOp::Mul => self.builder.build_basic_int_mul(lhs, rhs, "multmp"),
            BinaryOp::Div | BinaryOp::Rem if !is_vector => {
                return self.emit_checked_int_div_rem(span, lhs, op, rhs, is_signed);
            }
            BinaryOp::Div if is_signed => {
                self.builder.build_basic_int_signed_div(lhs, rhs, "divtmp")
            }
            BinaryOp::Div => self
                .builder
                .build_basic_int_unsigned_div(lhs, rhs, "divtmp"),
            BinaryOp::Rem if is_signed => {
                self.builder.build_basic_int_signed_rem(lhs, rhs, "remtmp")
            }
            BinaryOp::Rem => self
                .builder
                .build_basic_int_unsigned_rem(lhs, rhs, "remtmp"),
            BinaryOp::BitAnd => self.builder.build_basic_and(lhs, rhs, "andtmp"),
            BinaryOp::BitOr => self.builder.build_basic_or(lhs, rhs, "ortmp"),
            BinaryOp::BitXor => self.builder.build_basic_xor(lhs, rhs, "xortmp"),
            BinaryOp::Shl => {
                let rhs = self.normalize_shift_rhs(span, lhs, rhs)?;
                self.builder.build_basic_shl(lhs, rhs, "shltmp")
            }
            BinaryOp::Shr if is_signed => {
                let rhs = self.normalize_shift_rhs(span, lhs, rhs)?;
                self.builder.build_basic_ashr(lhs, rhs, "shrtmp")
            }
            BinaryOp::Shr => {
                let rhs = self.normalize_shift_rhs(span, lhs, rhs)?;
                self.builder.build_basic_lshr(lhs, rhs, "shrtmp")
            }
            BinaryOp::Eq => {
                self.builder
                    .build_basic_int_compare(IntPredicate::EQ, lhs, rhs, "eqtmp")
            }
            BinaryOp::Ne => {
                self.builder
                    .build_basic_int_compare(IntPredicate::NE, lhs, rhs, "netmp")
            }
            BinaryOp::Lt if is_signed => {
                self.builder
                    .build_basic_int_compare(IntPredicate::SLT, lhs, rhs, "lttmp")
            }
            BinaryOp::Lt => {
                self.builder
                    .build_basic_int_compare(IntPredicate::ULT, lhs, rhs, "lttmp")
            }
            BinaryOp::Le if is_signed => {
                self.builder
                    .build_basic_int_compare(IntPredicate::SLE, lhs, rhs, "letmp")
            }
            BinaryOp::Le => {
                self.builder
                    .build_basic_int_compare(IntPredicate::ULE, lhs, rhs, "letmp")
            }
            BinaryOp::Gt if is_signed => {
                self.builder
                    .build_basic_int_compare(IntPredicate::SGT, lhs, rhs, "gttmp")
            }
            BinaryOp::Gt => {
                self.builder
                    .build_basic_int_compare(IntPredicate::UGT, lhs, rhs, "gttmp")
            }
            BinaryOp::Ge if is_signed => {
                self.builder
                    .build_basic_int_compare(IntPredicate::SGE, lhs, rhs, "getmp")
            }
            BinaryOp::Ge => {
                self.builder
                    .build_basic_int_compare(IntPredicate::UGE, lhs, rhs, "getmp")
            }
            BinaryOp::And | BinaryOp::Or => {
                return Err(self.error(span, "logical operator reached non-short-circuit path"));
            }
        };
        result.map_err(|_| self.error(span, "failed to build binary operation"))
    }

    fn emit_checked_int_div_rem(
        &mut self,
        span: Span,
        lhs: BasicValueEnum<'ctx>,
        op: BinaryOp,
        rhs: BasicValueEnum<'ctx>,
        is_signed: bool,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let lhs_int = lhs.into_int_value()?;
        let rhs_int = rhs.into_int_value()?;
        let int_ty = lhs_int.get_type();
        let rhs_is_zero = self
            .builder
            .build_int_compare(
                IntPredicate::EQ,
                rhs_int,
                int_ty.const_zero(),
                "divrem.zero",
            )
            .map_err(|_| self.error(span, "failed to check integer divisor"))?;
        let must_trap = if is_signed {
            let bits = int_ty.bit_width();
            let min = int_ty.const_u128(1u128 << (bits - 1));
            let negative_one = int_ty.const_u128(u128::MAX);
            let lhs_is_min = self
                .builder
                .build_int_compare(IntPredicate::EQ, lhs_int, min, "divrem.min")
                .map_err(|_| self.error(span, "failed to check signed division lhs"))?;
            let rhs_is_negative_one = self
                .builder
                .build_int_compare(
                    IntPredicate::EQ,
                    rhs_int,
                    negative_one,
                    "divrem.negative_one",
                )
                .map_err(|_| self.error(span, "failed to check signed division rhs"))?;
            let signed_overflow = self
                .builder
                .build_basic_and(
                    lhs_is_min.into(),
                    rhs_is_negative_one.into(),
                    "divrem.overflow",
                )
                .map_err(|_| self.error(span, "failed to combine signed division checks"))?;
            self.builder
                .build_basic_or(rhs_is_zero.into(), signed_overflow, "divrem.traps")
                .map_err(|_| self.error(span, "failed to combine integer division checks"))?
                .into_int_value()?
        } else {
            rhs_is_zero
        };

        let trap_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "divrem.trap")?;
        let operation_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "divrem.operation")?;
        self.builder
            .build_conditional_branch(must_trap, trap_block, operation_block)
            .map_err(|_| self.error(span, "failed to branch on integer division checks"))?;

        self.builder.position_at_end(trap_block);
        self.emit_trap(span)?;
        self.builder.position_at_end(operation_block);
        let result = match (op, is_signed) {
            (BinaryOp::Div, true) => self.builder.build_basic_int_signed_div(lhs, rhs, "divtmp"),
            (BinaryOp::Div, false) => self
                .builder
                .build_basic_int_unsigned_div(lhs, rhs, "divtmp"),
            (BinaryOp::Rem, true) => self.builder.build_basic_int_signed_rem(lhs, rhs, "remtmp"),
            (BinaryOp::Rem, false) => self
                .builder
                .build_basic_int_unsigned_rem(lhs, rhs, "remtmp"),
            _ => unreachable!("only integer division and remainder reach checked div/rem codegen"),
        };
        result.map_err(|_| self.error(span, "failed to build checked integer division operation"))
    }

    fn normalize_shift_rhs(
        &self,
        span: Span,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let lhs = lhs.into_int_value()?;
        let rhs = rhs.into_int_value()?;
        let target = lhs.get_type();
        let target_bits = target.bit_width();
        let rhs_bits = rhs.get_type().bit_width();
        if rhs_bits == target_bits {
            return Ok(rhs.into());
        }
        let result = if rhs_bits > target_bits {
            self.builder.build_int_truncate(rhs, target, "shiftcount")
        } else {
            self.builder.build_int_z_extend(rhs, target, "shiftcount")
        };
        result
            .map(Into::into)
            .map_err(|_| self.error(span, "failed to cast shift count"))
    }

    fn emit_float_binary(
        &self,
        span: Span,
        lhs: BasicValueEnum<'ctx>,
        op: BinaryOp,
        rhs: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let result = match op {
            BinaryOp::Add => self.builder.build_basic_float_add(lhs, rhs, "faddtmp"),
            BinaryOp::Sub => self.builder.build_basic_float_sub(lhs, rhs, "fsubtmp"),
            BinaryOp::Mul => self.builder.build_basic_float_mul(lhs, rhs, "fmultmp"),
            BinaryOp::Div => self.builder.build_basic_float_div(lhs, rhs, "fdivtmp"),
            BinaryOp::Rem => self.builder.build_basic_float_rem(lhs, rhs, "fremtmp"),
            BinaryOp::Eq => {
                self.builder
                    .build_basic_float_compare(FloatPredicate::OEQ, lhs, rhs, "feqtmp")
            }
            BinaryOp::Ne => {
                self.builder
                    .build_basic_float_compare(FloatPredicate::ONE, lhs, rhs, "fnetmp")
            }
            BinaryOp::Lt => {
                self.builder
                    .build_basic_float_compare(FloatPredicate::OLT, lhs, rhs, "flttmp")
            }
            BinaryOp::Le => {
                self.builder
                    .build_basic_float_compare(FloatPredicate::OLE, lhs, rhs, "fletmp")
            }
            BinaryOp::Gt => {
                self.builder
                    .build_basic_float_compare(FloatPredicate::OGT, lhs, rhs, "fgttmp")
            }
            BinaryOp::Ge => {
                self.builder
                    .build_basic_float_compare(FloatPredicate::OGE, lhs, rhs, "fgetmp")
            }
            BinaryOp::Shl
            | BinaryOp::Shr
            | BinaryOp::BitAnd
            | BinaryOp::BitXor
            | BinaryOp::BitOr
            | BinaryOp::And
            | BinaryOp::Or => {
                return Err(self.error(span, "operator is not valid for float operands"));
            }
        };
        result.map_err(|_| self.error(span, "failed to build float binary operation"))
    }
}
