// SPDX-License-Identifier: GPL-3.0-or-later
//! Failed runtime checks.
//!
//! Code of a module whose `trap_locations` are known reports a failed check
//! through the runtime's `__nia_trap` (see `lib/runtime/builtins/trap.nia`
//! and "Trap Reports" in the ABI reference): the check's location, what it
//! tested, the operand type, and the operands. Other code, and code of the
//! runtime itself, executes the target's trap instruction directly. Both end
//! the program the same way; a report only adds one line on standard error.

use super::FunctionCodegen;
use nia_diagnostic::Diagnostic;
use nia_ids::InternedTyId;
use nia_llvm::module::Linkage;
use nia_llvm::values::{BasicValueEnum, IntValue};
use nia_llvm::{Attribute, AttributeLoc};
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyKind};

/// What a failed check tested, in the runtime's numbering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TrapCause {
    Add = 1,
    Sub,
    Mul,
    Neg,
    Div,
    Rem,
    ShlAmount,
    ShrAmount,
    ShlOverflow,
    LaneIndex,
    Explicit,
    Index,
    SliceEnd,
    SliceInclusiveEnd,
    SliceOrder,
    SliceInclusiveOrder,
}

/// One operand of a failed check and whether its type is signed. A vector
/// operand reports the lane that failed.
#[derive(Clone, Copy)]
pub(super) struct TrapOperand<'ctx> {
    pub(super) value: BasicValueEnum<'ctx>,
    pub(super) signed: bool,
}

/// A failed check: its cause and, when the cause has them, the operation's
/// integer or integer-vector type and its operands.
pub(super) struct TrapCheck<'ctx> {
    cause: TrapCause,
    ty: Option<InternedTyId>,
    lhs: Option<TrapOperand<'ctx>>,
    rhs: Option<TrapOperand<'ctx>>,
    /// For a vector operation, the mask of lanes that failed.
    failed_lanes: Option<BasicValueEnum<'ctx>>,
}

impl<'ctx> TrapCheck<'ctx> {
    /// `std::builtin::trap()`.
    pub(super) fn explicit() -> Self {
        Self {
            cause: TrapCause::Explicit,
            ty: None,
            lhs: None,
            rhs: None,
            failed_lanes: None,
        }
    }

    /// A check of an operation on values of type `ty`.
    pub(super) fn operation(
        cause: TrapCause,
        ty: InternedTyId,
        lhs: TrapOperand<'ctx>,
        rhs: TrapOperand<'ctx>,
    ) -> Self {
        Self {
            cause,
            ty: Some(ty),
            lhs: Some(lhs),
            rhs: Some(rhs),
            failed_lanes: None,
        }
    }

    /// An `extract` or `insert` lane index outside the vector type `ty`.
    pub(super) fn lane_index(ty: InternedTyId, index: BasicValueEnum<'ctx>) -> Self {
        Self {
            cause: TrapCause::LaneIndex,
            ty: Some(ty),
            lhs: Some(TrapOperand {
                value: index,
                signed: false,
            }),
            rhs: None,
            failed_lanes: None,
        }
    }

    /// A position checked against a bound, both `usize`: an index against a
    /// length, a slice end against a length, or a slice start against its end.
    pub(super) fn bound(cause: TrapCause, value: IntValue<'ctx>, bound: IntValue<'ctx>) -> Self {
        Self {
            cause,
            ty: None,
            lhs: Some(TrapOperand {
                value: value.into(),
                signed: false,
            }),
            rhs: Some(TrapOperand {
                value: bound.into(),
                signed: false,
            }),
            failed_lanes: None,
        }
    }

    /// Restricts a vector check's report to the first lane in `mask`.
    pub(super) fn in_lanes(mut self, mask: BasicValueEnum<'ctx>) -> Self {
        self.failed_lanes = Some(mask);
        self
    }
}

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    /// Continues only when `value < bound` (or `value <= bound` when
    /// `inclusive`), both unsigned; otherwise fails with `cause`.
    pub(super) fn emit_bound_check(
        &mut self,
        span: Span,
        cause: TrapCause,
        value: IntValue<'ctx>,
        bound: IntValue<'ctx>,
        inclusive: bool,
    ) -> Result<(), Diagnostic> {
        let predicate = if inclusive {
            nia_llvm::IntPredicate::ULE
        } else {
            nia_llvm::IntPredicate::ULT
        };
        let in_bounds = self
            .builder
            .build_int_compare(predicate, value, bound, "bounds.ok")
            .map_err(|_| self.error(span, "failed to compare a bound"))?;
        let context = self.module.context;
        let fail = context.append_basic_block(self.llvm_function, "bounds.fail")?;
        let ok = context.append_basic_block(self.llvm_function, "bounds.ok")?;
        self.builder
            .build_conditional_branch(in_bounds, ok, fail)
            .map_err(|_| self.error(span, "failed to branch on a bound"))?;
        self.builder.position_at_end(fail);
        self.emit_trap(span, TrapCheck::bound(cause, value, bound))?;
        self.builder.position_at_end(ok);
        Ok(())
    }

    /// Ends the current block with the failure of `check`.
    pub(super) fn emit_trap(
        &mut self,
        span: Span,
        check: TrapCheck<'ctx>,
    ) -> Result<(), Diagnostic> {
        if self.current_block_has_terminator() {
            return Ok(());
        }
        if self.owner_trap_locations().is_some() {
            self.emit_trap_report(span, check)?;
        } else {
            self.emit_trap_instruction(span)?;
        }
        if !self.current_block_has_terminator() {
            self.builder
                .build_unreachable()
                .map_err(|_| self.error(span, "failed to build trap terminator"))?;
        }
        Ok(())
    }

    fn emit_trap_instruction(&mut self, span: Span) -> Result<(), Diagnostic> {
        let intrinsic = nia_llvm::intrinsics::Intrinsic::find("llvm.trap")
            .and_then(|intrinsic| intrinsic.get_declaration(&self.module.module, &[]))
            .ok_or_else(|| self.error(span, "failed to declare trap intrinsic"))?;
        self.builder
            .build_call(intrinsic, &[], "trap")
            .map_err(|_| self.error(span, "failed to build trap call"))?;
        Ok(())
    }

    /// The span resolution of the module that owns the function being
    /// generated. Copied and instantiated bodies keep their own module's.
    fn owner_trap_locations(&self) -> Option<&'a nia_source::SourceLineIndex> {
        let module_id = match &self.function.closure_owner {
            nia_backend_ir::BackendClosureEntryOwner::Source(def_id) => def_id.module_id,
            nia_backend_ir::BackendClosureEntryOwner::FunctionInstance(key) => key.def_id.module_id,
        };
        self.module
            .program
            .module(module_id)
            .and_then(|module| module.trap_locations.as_deref())
    }

    fn emit_trap_report(&mut self, span: Span, check: TrapCheck<'ctx>) -> Result<(), Diagnostic> {
        let context = self.module.context;
        let i32_ty = context.i32_type();
        let i64_ty = context.i64_type();
        let location = match self.caller_location {
            // A tracked function reports where it was called, as
            // `callerLocation()` does inside it.
            Some(location) => location,
            None => {
                let locations = self
                    .owner_trap_locations()
                    .ok_or_else(|| self.error(span, "trap report without locations"))?;
                self.module
                    .materialize_source_location(&locations.location(span.start), span)?
            }
        };
        let (info, lanes) = self.trap_info(span, &check)?;
        let lane = match (check.failed_lanes, lanes) {
            (Some(mask), Some(lanes)) => Some(self.first_failed_lane(span, mask, lanes)?),
            _ => None,
        };
        let mut args: Vec<BasicValueEnum<'ctx>> = vec![
            location.into(),
            i32_ty.const_int(u64::from(info), false)?.into(),
            match lane {
                Some(lane) => lane.into(),
                None => i32_ty.const_zero()?.into(),
            },
        ];
        for operand in [check.lhs, check.rhs] {
            let (low, high) = match operand {
                Some(operand) => self.trap_operand_halves(span, operand, lane)?,
                None => (i64_ty.const_zero()?, i64_ty.const_zero()?),
            };
            args.push(low.into());
            args.push(high.into());
        }
        let reporter = self.trap_reporter()?;
        let call = self
            .builder
            .build_call(reporter, &args, "")
            .map_err(|_| self.error(span, "failed to build trap report call"))?;
        for attribute in ["noreturn", "cold", "nounwind"] {
            call.add_attribute(
                AttributeLoc::Function,
                context.create_enum_attribute(Attribute::get_named_enum_kind_id(attribute), 0)?,
            );
        }
        Ok(())
    }

    /// The runtime reporter, declared on first use. Its C signature is
    /// `void __nia_trap(const SourceLocation *, uint32_t info, uint32_t lane,
    /// uint64_t lhs_low, uint64_t lhs_high, uint64_t rhs_low, uint64_t
    /// rhs_high)`; it never returns.
    fn trap_reporter(&self) -> Result<nia_llvm::values::FunctionValue<'ctx>, Diagnostic> {
        let module = &self.module.module;
        if let Some(function) = module
            .get_function(nia_toolchain::TRAP_REPORT_SYMBOL)
            .map_err(super::ModuleCodegen::diagnostic_from_llvm_error)?
        {
            return Ok(function);
        }
        let context = self.module.context;
        let i32_ty = context.i32_type();
        let i64_ty = context.i64_type();
        let ty = context
            .void_type()
            .fn_type(
                &[
                    context.ptr_type(nia_llvm::AddressSpace(0)).into(),
                    i32_ty.into(),
                    i32_ty.into(),
                    i64_ty.into(),
                    i64_ty.into(),
                    i64_ty.into(),
                    i64_ty.into(),
                ],
                false,
            )
            .map_err(super::ModuleCodegen::diagnostic_from_llvm_error)?;
        let function = module
            .add_function(
                nia_toolchain::TRAP_REPORT_SYMBOL,
                ty,
                Some(Linkage::External),
            )
            .map_err(super::ModuleCodegen::diagnostic_from_llvm_error)?;
        for attribute in ["noreturn", "cold", "nounwind"] {
            function.add_attribute(
                AttributeLoc::Function,
                context
                    .create_enum_attribute(Attribute::get_named_enum_kind_id(attribute), 0)
                    .map_err(super::ModuleCodegen::diagnostic_from_llvm_error)?,
            );
        }
        Ok(function)
    }

    /// The `info` word: cause in bits 0-7, lane width class in bits 8-11,
    /// lane signedness in bit 12, the left operand's signedness in bit 13,
    /// lane count in bits 16-30 (0 for a scalar), and the right operand's
    /// signedness in bit 31. Also returns the lane count of a vector
    /// operation.
    fn trap_info(
        &self,
        span: Span,
        check: &TrapCheck<'ctx>,
    ) -> Result<(u32, Option<u32>), Diagnostic> {
        let mut info = check.cause as u32;
        let Some(ty) = check.ty else {
            return Ok((info, None));
        };
        let (element, lanes) = match self.module.ty_kind(ty) {
            Some(TyKind::Primitive(primitive)) => (*primitive, None),
            Some(TyKind::Vector { elem, lanes }) => (*elem, Some(*lanes)),
            other => {
                return Err(self.error(
                    span,
                    format!("runtime check on a non-integer type: {other:?}"),
                ));
            }
        };
        let width = match element {
            PrimitiveTy::I8 | PrimitiveTy::U8 => 0,
            PrimitiveTy::I16 | PrimitiveTy::U16 => 1,
            PrimitiveTy::I32 | PrimitiveTy::U32 => 2,
            PrimitiveTy::I64 | PrimitiveTy::U64 => 3,
            PrimitiveTy::I128 | PrimitiveTy::U128 => 4,
            PrimitiveTy::Isize | PrimitiveTy::Usize => 5,
            // Only a lane index check of a mask vector names a `bool` lane.
            PrimitiveTy::Bool => 6,
            other => {
                return Err(self.error(
                    span,
                    format!("runtime check on a non-integer lane type: {other:?}"),
                ));
            }
        };
        info |= width << 8;
        info |= u32::from(element.is_signed_integer()) << 12;
        if let Some(lanes) = lanes {
            info |= (lanes & 0x7fff) << 16;
        }
        if check.lhs.is_some_and(|lhs| lhs.signed) {
            info |= 1 << 13;
        }
        if check.rhs.is_some_and(|rhs| rhs.signed) {
            info |= 1 << 31;
        }
        Ok((info, lanes))
    }

    /// The index of the lowest set lane of `mask`, an `<N x i1>`.
    fn first_failed_lane(
        &mut self,
        span: Span,
        mask: BasicValueEnum<'ctx>,
        lanes: u32,
    ) -> Result<IntValue<'ctx>, Diagnostic> {
        let context = self.module.context;
        let bits_ty = context.custom_width_int_type(lanes).map_err(|error| {
            self.error(span, format!("failed to create lane mask type: {error:?}"))
        })?;
        let bits = self
            .builder
            .build_bit_cast(mask, bits_ty, "trap.lanes")
            .map_err(|_| self.error(span, "failed to read the failed lanes"))?
            .into_int_value()?;
        let cttz = nia_llvm::intrinsics::Intrinsic::find("llvm.cttz")
            .and_then(|intrinsic| intrinsic.get_declaration(&self.module.module, &[bits_ty.into()]))
            .ok_or_else(|| self.error(span, "failed to declare cttz"))?;
        let first = self
            .builder
            .build_call(
                cttz,
                &[bits.into(), context.bool_type().const_int(1, false)?.into()],
                "trap.lane",
            )
            .map_err(|_| self.error(span, "failed to find the failed lane"))?
            .try_as_basic_value()
            .unwrap_basic()
            .map_err(|_| self.error(span, "cttz returned no value"))?
            .into_int_value()?;
        let i32_ty = context.i32_type();
        let first = match lanes.cmp(&32) {
            std::cmp::Ordering::Less => self.builder.build_int_z_extend(first, i32_ty, "trap.lane"),
            std::cmp::Ordering::Equal => Ok(first),
            std::cmp::Ordering::Greater => {
                self.builder.build_int_truncate(first, i32_ty, "trap.lane")
            }
        }
        .map_err(|_| self.error(span, "failed to convert the failed lane"))?;
        Ok(first)
    }

    /// The operand widened to 128 bits by its signedness, as low and high
    /// halves. A vector operand gives its `lane`.
    fn trap_operand_halves(
        &mut self,
        span: Span,
        operand: TrapOperand<'ctx>,
        lane: Option<IntValue<'ctx>>,
    ) -> Result<(IntValue<'ctx>, IntValue<'ctx>), Diagnostic> {
        let context = self.module.context;
        let value = match (operand.value, lane) {
            (BasicValueEnum::VectorValue(vector), Some(lane)) => self
                .builder
                .build_extract_element(vector, lane, "trap.operand")
                .map_err(|_| self.error(span, "failed to read the failed lane's operand"))?
                .into_int_value()?,
            (value, _) => value.into_int_value()?,
        };
        let i128_ty = context.i128_type();
        let wide = match value.get_type()?.bit_width().cmp(&128) {
            std::cmp::Ordering::Less if operand.signed => {
                self.builder.build_int_s_extend(value, i128_ty, "trap.wide")
            }
            std::cmp::Ordering::Less => {
                self.builder.build_int_z_extend(value, i128_ty, "trap.wide")
            }
            _ => Ok(value),
        }
        .map_err(|_| self.error(span, "failed to widen a trap operand"))?;
        let i64_ty = context.i64_type();
        let low = self
            .builder
            .build_int_truncate(wide, i64_ty, "trap.low")
            .map_err(|_| self.error(span, "failed to split a trap operand"))?;
        let shifted = self
            .builder
            .build_right_shift(wide, i128_ty.const_int(64, false)?, false, "trap.high")
            .map_err(|_| self.error(span, "failed to split a trap operand"))?;
        let high = self
            .builder
            .build_int_truncate(shifted, i64_ty, "trap.high")
            .map_err(|_| self.error(span, "failed to split a trap operand"))?;
        Ok((low, high))
    }
}
