// SPDX-License-Identifier: GPL-3.0-or-later
//! Compiler-rt shift libcalls. Each wide value is split into two half-width
//! limbs so the helper cannot lower to a call to itself. On 32-bit targets the
//! i128 helpers may call the i64 helpers; builtin discovery closes that dependency.

use nia_llvm::{
    Context, IntPredicate, LlvmError, LlvmResult,
    module::{Linkage, Module},
    types::BasicTypeEnum,
    values::IntValue,
};

use super::hide_builtin;

#[derive(Clone, Copy)]
enum Shift {
    Left,
    LogicalRight,
    ArithmeticRight,
}

pub(super) fn emit<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    bits: u32,
    windows_i128_abi: bool,
) -> LlvmResult<()> {
    let names = match bits {
        64 => ["__ashldi3", "__lshrdi3", "__ashrdi3"],
        128 => ["__ashlti3", "__lshrti3", "__ashrti3"],
        _ => return Err(LlvmError::ice("unsupported shift builtin width")),
    };
    for (name, shift) in
        names
            .into_iter()
            .zip([Shift::Left, Shift::LogicalRight, Shift::ArithmeticRight])
    {
        emit_one(context, module, name, bits, windows_i128_abi, shift)?;
    }
    Ok(())
}

fn emit_one<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    name: &str,
    bits: u32,
    windows_i128_abi: bool,
    shift: Shift,
) -> LlvmResult<()> {
    let wide_ty = context.custom_width_int_type(bits)?;
    let half_bits = bits / 2;
    let limb_ty = context.custom_width_int_type(half_bits)?;
    let limbs_ty = BasicTypeEnum::from(limb_ty).vector_type(2)?;
    let windows_abi = windows_i128_abi && bits == 128;
    let result_ty: BasicTypeEnum<'ctx> = if windows_abi {
        limbs_ty.into()
    } else {
        wide_ty.into()
    };
    let argument_ty: BasicTypeEnum<'ctx> = if windows_abi {
        context.ptr_type(nia_llvm::AddressSpace(0)).into()
    } else {
        wide_ty.into()
    };
    let function = module.add_function(
        name,
        result_ty.fn_type(&[argument_ty, context.i32_type().into()], false)?,
        Some(Linkage::External),
    )?;
    hide_builtin(function);
    let builder = context.create_builder()?;
    let entry = context.append_basic_block(function, "entry")?;
    let zero_count = context.append_basic_block(function, "zero")?;
    let nonzero = context.append_basic_block(function, "nonzero")?;
    let small = context.append_basic_block(function, "small")?;
    let large_dispatch = context.append_basic_block(function, "large.dispatch")?;
    let large = context.append_basic_block(function, "large")?;
    let too_large = context.append_basic_block(function, "too.large")?;
    builder.position_at_end(entry);
    let arg = function
        .get_nth_param(0)
        .ok_or_else(|| LlvmError::ice("missing shift argument"))??;
    let wide = if windows_abi {
        builder.build_load(wide_ty, arg.into_pointer_value()?, "arg")?
    } else {
        arg
    };
    let count = function
        .get_nth_param(1)
        .ok_or_else(|| LlvmError::ice("missing shift count"))??
        .into_int_value()?;
    // Every maintained Nia target is little endian. LLVM's integer/vector
    // bitcast therefore places the low half in lane zero, without wide shifts.
    let limbs = builder
        .build_bit_cast(wide, limbs_ty, "limbs")?
        .into_vector_value()?;
    let low = builder
        .build_extract_element(limbs, context.i32_type().const_int(0, false)?, "low")?
        .into_int_value()?;
    let high = builder
        .build_extract_element(limbs, context.i32_type().const_int(1, false)?, "high")?
        .into_int_value()?;
    let count = if half_bits == 32 {
        count
    } else {
        builder.build_int_z_extend(count, limb_ty, "count")?
    };
    let zero = limb_ty.const_zero()?;
    let half = limb_ty.const_int(u64::from(half_bits), false)?;
    let is_zero = builder.build_int_compare(IntPredicate::EQ, count, zero, "is.zero")?;
    builder.build_conditional_branch(is_zero, zero_count, nonzero)?;

    let finish = |low: IntValue<'ctx>, high: IntValue<'ctx>| -> LlvmResult<()> {
        let lanes = limbs_ty.const_zero()?.into_vector_value()?;
        let lanes = builder
            .build_insert_element(
                lanes,
                low.into(),
                context.i32_type().const_int(0, false)?,
                "result.low",
            )?
            .into_vector_value()?;
        let result = builder.build_insert_element(
            lanes,
            high.into(),
            context.i32_type().const_int(1, false)?,
            "result.high",
        )?;
        let result = builder.build_bit_cast(result, result_ty, "result")?;
        builder.build_return(Some(&result))?;
        Ok(())
    };
    builder.position_at_end(zero_count);
    finish(low, high)?;

    builder.position_at_end(nonzero);
    let at_least_half =
        builder.build_int_compare(IntPredicate::UGE, count, half, "at.least.half")?;
    builder.build_conditional_branch(at_least_half, large_dispatch, small)?;

    builder.position_at_end(large_dispatch);
    let at_least_total = builder.build_int_compare(
        IntPredicate::UGE,
        count,
        limb_ty.const_int(u64::from(bits), false)?,
        "at.least.total",
    )?;
    builder.build_conditional_branch(at_least_total, too_large, large)?;

    // The compiler-rt contract has 0 <= count < bits. Keeping count=0 out of
    // this block avoids a poison shift by half_bits in the carry expression.
    builder.position_at_end(small);
    let reverse = builder.build_int_sub(half, count, "reverse")?;
    match shift {
        Shift::Left => {
            let carry = builder.build_right_shift(low, reverse, false, "carry")?;
            let low = builder.build_left_shift(low, count, "low.shift")?;
            let high = builder.build_left_shift(high, count, "high.shift")?;
            finish(low, builder.build_or(high, carry, "high.carry")?)?;
        }
        Shift::LogicalRight | Shift::ArithmeticRight => {
            let carry = builder.build_left_shift(high, reverse, "carry")?;
            let low = builder.build_right_shift(low, count, false, "low.shift")?;
            let high = builder.build_right_shift(
                high,
                count,
                matches!(shift, Shift::ArithmeticRight),
                "high.shift",
            )?;
            finish(builder.build_or(low, carry, "low.carry")?, high)?;
        }
    }

    builder.position_at_end(large);
    let remaining = builder.build_int_sub(count, half, "remaining")?;
    match shift {
        Shift::Left => finish(
            zero,
            builder.build_left_shift(low, remaining, "high.shift")?,
        )?,
        Shift::LogicalRight => finish(
            builder.build_right_shift(high, remaining, false, "low.shift")?,
            zero,
        )?,
        Shift::ArithmeticRight => {
            let low = builder.build_right_shift(high, remaining, true, "low.shift")?;
            let sign = builder.build_right_shift(
                high,
                limb_ty.const_int(u64::from(half_bits - 1), false)?,
                true,
                "sign",
            )?;
            finish(low, sign)?;
        }
    }

    builder.position_at_end(too_large);
    match shift {
        Shift::ArithmeticRight => {
            let sign = builder.build_right_shift(
                high,
                limb_ty.const_int(u64::from(half_bits - 1), false)?,
                true,
                "sign",
            )?;
            finish(sign, sign)?;
        }
        Shift::Left | Shift::LogicalRight => finish(zero, zero)?,
    }
    Ok(())
}
