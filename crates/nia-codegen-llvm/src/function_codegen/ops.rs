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
                    let is_vector =
                        matches!(self.module.ty_kind(ty), Some(nia_ty::TyKind::Vector { .. }));
                    if is_vector {
                        let lanes = match self.module.ty_kind(ty) {
                            Some(nia_ty::TyKind::Vector { lanes, .. }) => *lanes,
                            _ => unreachable!("vector negation requires a vector type"),
                        };
                        let zero = value
                            .get_type()?
                            .const_zero()
                            .map_err(|_| self.error(span, "failed to create vector zero"))?;
                        self.emit_checked_int_arithmetic(
                            span,
                            zero,
                            BinaryOp::Sub,
                            value,
                            self.is_signed_integer(ty),
                            Some(lanes),
                        )
                    } else {
                        let int_ty = value.into_int_value()?.get_type();
                        self.emit_checked_int_arithmetic(
                            span,
                            int_ty.const_zero().into(),
                            BinaryOp::Sub,
                            value,
                            self.is_signed_integer(ty),
                            None,
                        )
                    }
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
        rhs_ty: InternedTyId,
        rhs: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(op) = assign_to_binary_op(op) else {
            return Ok(rhs);
        };
        self.emit_binary(span, operand_ty, lhs, op, rhs_ty, rhs)
    }

    pub(super) fn emit_binary(
        &mut self,
        span: Span,
        operand_ty: InternedTyId,
        lhs: BasicValueEnum<'ctx>,
        op: BinaryOp,
        rhs_ty: InternedTyId,
        rhs: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        if self.is_float(operand_ty) {
            return self.emit_float_binary(span, lhs, op, rhs);
        }
        let is_signed = self.is_signed_integer(operand_ty);
        let vector_lanes = match self.module.ty_kind(operand_ty) {
            Some(nia_ty::TyKind::Vector { lanes, .. }) => Some(*lanes),
            _ => None,
        };
        let is_vector = vector_lanes.is_some();
        let result = match op {
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul => {
                return self.emit_checked_int_arithmetic(
                    span,
                    lhs,
                    op,
                    rhs,
                    is_signed,
                    vector_lanes,
                );
            }
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
            BinaryOp::Shl if !is_vector => {
                return self.emit_checked_int_shift(span, lhs, op, rhs_ty, rhs, is_signed);
            }
            BinaryOp::Shr if !is_vector => {
                return self.emit_checked_int_shift(span, lhs, op, rhs_ty, rhs, is_signed);
            }
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

    fn emit_checked_int_shift(
        &mut self,
        span: Span,
        lhs: BasicValueEnum<'ctx>,
        op: BinaryOp,
        rhs_ty: InternedTyId,
        rhs: BasicValueEnum<'ctx>,
        lhs_is_signed: bool,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let lhs = lhs.into_int_value()?;
        let rhs = rhs.into_int_value()?;
        let lhs_ty = lhs.get_type();
        let lhs_bits = lhs_ty.bit_width();
        let rhs_bits = rhs.get_type().bit_width();
        let check_ty = self.module.context.custom_width_int_type(rhs_bits.max(8));
        let checked_rhs = if rhs_bits == check_ty.bit_width() {
            rhs
        } else {
            self.builder
                .build_int_z_extend(rhs, check_ty, "shift.count.check")
                .map_err(|_| self.error(span, "failed to extend shift count for validation"))?
        };
        let count_out_of_range = self
            .builder
            .build_int_compare(
                IntPredicate::UGE,
                checked_rhs,
                check_ty.const_int(u64::from(lhs_bits), false),
                "shift.count.out_of_range",
            )
            .map_err(|_| self.error(span, "failed to validate shift count range"))?;
        let invalid_count = if self.is_signed_integer(rhs_ty) {
            let count_is_negative = self
                .builder
                .build_int_compare(
                    IntPredicate::SLT,
                    rhs,
                    rhs.get_type().const_zero(),
                    "shift.count.negative",
                )
                .map_err(|_| self.error(span, "failed to validate signed shift count"))?;
            self.builder
                .build_basic_or(
                    count_is_negative.into(),
                    count_out_of_range.into(),
                    "shift.count.invalid",
                )
                .map_err(|_| self.error(span, "failed to combine shift count checks"))?
                .into_int_value()?
        } else {
            count_out_of_range
        };

        let count_trap = self
            .module
            .context
            .append_basic_block(self.llvm_function, "shift.count.trap")?;
        let operation = self
            .module
            .context
            .append_basic_block(self.llvm_function, "shift.operation")?;
        self.builder
            .build_conditional_branch(invalid_count, count_trap, operation)
            .map_err(|_| self.error(span, "failed to branch on shift count validation"))?;
        self.builder.position_at_end(count_trap);
        self.emit_trap(span)?;
        self.builder.position_at_end(operation);

        let rhs = self.normalize_shift_rhs(span, lhs.into(), rhs.into())?;
        if op == BinaryOp::Shr {
            let result = if lhs_is_signed {
                self.builder.build_basic_ashr(lhs.into(), rhs, "shrtmp")
            } else {
                self.builder.build_basic_lshr(lhs.into(), rhs, "shrtmp")
            };
            return result.map_err(|_| self.error(span, "failed to build checked right shift"));
        }

        let wide_ty = self.module.context.custom_width_int_type(lhs_bits * 2);
        let wide_lhs = if lhs_is_signed {
            self.builder
                .build_int_s_extend(lhs, wide_ty, "shift.lhs.wide")
        } else {
            self.builder
                .build_int_z_extend(lhs, wide_ty, "shift.lhs.wide")
        }
        .map_err(|_| self.error(span, "failed to extend left shift operand"))?;
        let rhs = rhs.into_int_value()?;
        let wide_rhs = self
            .builder
            .build_int_z_extend(rhs, wide_ty, "shift.count.wide")
            .map_err(|_| self.error(span, "failed to extend left shift count"))?;
        let wide_result = self
            .builder
            .build_basic_shl(wide_lhs.into(), wide_rhs.into(), "shift.result.wide")
            .map_err(|_| self.error(span, "failed to build wide left shift"))?
            .into_int_value()?;
        let result = self
            .builder
            .build_int_truncate(wide_result, lhs_ty, "shltmp")
            .map_err(|_| self.error(span, "failed to truncate left shift result"))?;
        let restored = if lhs_is_signed {
            self.builder
                .build_int_s_extend(result, wide_ty, "shift.result.restored")
        } else {
            self.builder
                .build_int_z_extend(result, wide_ty, "shift.result.restored")
        }
        .map_err(|_| self.error(span, "failed to validate left shift result"))?;
        let overflow = self
            .builder
            .build_int_compare(
                IntPredicate::NE,
                wide_result,
                restored,
                "shift.result.overflow",
            )
            .map_err(|_| self.error(span, "failed to compare left shift result"))?;
        let overflow_trap = self
            .module
            .context
            .append_basic_block(self.llvm_function, "shift.overflow.trap")?;
        let r#continue = self
            .module
            .context
            .append_basic_block(self.llvm_function, "shift.continue")?;
        self.builder
            .build_conditional_branch(overflow, overflow_trap, r#continue)
            .map_err(|_| self.error(span, "failed to branch on left shift overflow"))?;
        self.builder.position_at_end(overflow_trap);
        self.emit_trap(span)?;
        self.builder.position_at_end(r#continue);
        Ok(result.into())
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

    fn emit_checked_int_arithmetic(
        &mut self,
        span: Span,
        lhs: BasicValueEnum<'ctx>,
        op: BinaryOp,
        rhs: BasicValueEnum<'ctx>,
        is_signed: bool,
        vector_lanes: Option<u32>,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let intrinsic_name = match (op, is_signed) {
            (BinaryOp::Add, true) => "llvm.sadd.with.overflow",
            (BinaryOp::Add, false) => "llvm.uadd.with.overflow",
            (BinaryOp::Sub, true) => "llvm.ssub.with.overflow",
            (BinaryOp::Sub, false) => "llvm.usub.with.overflow",
            (BinaryOp::Mul, true) => "llvm.smul.with.overflow",
            (BinaryOp::Mul, false) => "llvm.umul.with.overflow",
            _ => unreachable!("only integer add/sub/mul reach checked arithmetic codegen"),
        };
        let ty = lhs.get_type()?;
        let intrinsic = nia_llvm::intrinsics::Intrinsic::find(intrinsic_name)
            .and_then(|intrinsic| intrinsic.get_declaration(&self.module.module, &[ty]))
            .ok_or_else(|| self.error(span, "failed to declare integer overflow intrinsic"))?;
        let call = self
            .builder
            .build_call(intrinsic, &[lhs, rhs], "arith.checked")
            .map_err(|_| self.error(span, "failed to build checked integer arithmetic"))?;
        let result = call
            .try_as_basic_value()
            .unwrap_basic()
            .map_err(|_| self.error(span, "integer overflow intrinsic returned no value"))?
            .into_struct_value()?;
        let value = self
            .builder
            .build_extract_value(result, 0, "arith.value")
            .map_err(|_| self.error(span, "failed to extract checked arithmetic value"))?;
        let overflow = self
            .builder
            .build_extract_value(result, 1, "arith.overflow")
            .map_err(|_| self.error(span, "failed to extract integer overflow flag"))?;
        let overflow = if let Some(lanes) = vector_lanes {
            self.reduce_vector_mask_any(span, overflow, lanes, "arith.overflow.any")?
        } else {
            overflow.into_int_value()?
        };

        let trap_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "arith.trap")?;
        let continue_block = self
            .module
            .context
            .append_basic_block(self.llvm_function, "arith.continue")?;
        self.builder
            .build_conditional_branch(overflow, trap_block, continue_block)
            .map_err(|_| self.error(span, "failed to branch on integer overflow"))?;
        self.builder.position_at_end(trap_block);
        self.emit_trap(span)?;
        self.builder.position_at_end(continue_block);
        Ok(value)
    }

    fn reduce_vector_mask_any(
        &self,
        span: Span,
        mask: BasicValueEnum<'ctx>,
        lanes: u32,
        name: &str,
    ) -> Result<nia_llvm::values::IntValue<'ctx>, Diagnostic> {
        let packed_ty = self.module.context.custom_width_int_type(lanes);
        let packed = self
            .builder
            .build_bit_cast(mask, packed_ty, "vector.mask.pack")
            .map_err(|_| self.error(span, "failed to pack vector condition mask"))?
            .into_int_value()?;
        self.builder
            .build_int_compare(IntPredicate::NE, packed, packed_ty.const_zero(), name)
            .map_err(|_| self.error(span, "failed to reduce vector condition mask"))
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
