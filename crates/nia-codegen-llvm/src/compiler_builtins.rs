// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{AssignOp, BinaryOp};
use nia_backend_ir::BackendProgram;
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    FunctionArrayElements, FunctionBody, FunctionBuiltinOperatorOp, FunctionCallee,
    FunctionDeferBody, FunctionExpr, FunctionExprKind, FunctionForHeader,
    FunctionMemoryIntrinsicSource, FunctionOp, FunctionPlace, FunctionPlaceBase, FunctionPlaceElem,
    FunctionSliceRange, FunctionTerminator,
};
use nia_llvm::{
    Context, IntPredicate, LlvmError,
    module::Linkage,
    target::TargetMachine,
    values::{BasicMetadataValueEnum, BasicValueEnum, IntValue, PointerValue},
};
use nia_ty::{PrimitiveTy, TyKind};

use crate::program_index::ProgramIndex;

pub(crate) fn required_symbols(
    program: &BackendProgram,
    index: &ProgramIndex<'_>,
) -> CompilerBuiltinSymbols {
    let mut collector = CompilerBuiltinCollector::default();
    collector.collect_program(program, index);
    collector.symbols
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CompilerBuiltinSymbols {
    pub(crate) u128_div_rem: bool,
    pub(crate) i128_div_rem: bool,
}

impl CompilerBuiltinSymbols {
    pub(crate) fn any(self) -> bool {
        self.u128_div_rem || self.i128_div_rem
    }
}

#[derive(Debug, Default)]
struct CompilerBuiltinCollector {
    symbols: CompilerBuiltinSymbols,
}

impl CompilerBuiltinCollector {
    fn collect_program(&mut self, program: &BackendProgram, index: &ProgramIndex<'_>) {
        for module in &program.modules {
            for function in &module.functions {
                if let Some(body) = &function.function_body {
                    self.collect_body(index, body);
                }
            }
            for function in &module.function_instances {
                if let Some(body) = &function.function_body {
                    self.collect_body(index, body);
                }
            }
        }
    }

    fn collect_body(&mut self, index: &ProgramIndex<'_>, body: &FunctionBody) {
        for block in &body.blocks {
            for op in &block.ops {
                self.collect_op(index, op);
            }
            self.collect_terminator(index, &block.terminator);
        }
    }

    fn collect_defer_body(&mut self, index: &ProgramIndex<'_>, body: &FunctionDeferBody) {
        for block in &body.blocks {
            for op in &block.ops {
                self.collect_op(index, op);
            }
            self.collect_terminator(index, &block.terminator);
        }
    }

    fn collect_op(&mut self, index: &ProgramIndex<'_>, op: &FunctionOp) {
        match op {
            FunctionOp::Binding(binding) => {
                if let Some(value) = &binding.value {
                    self.collect_expr(index, value);
                }
            }
            FunctionOp::StoreLocal { value, .. } | FunctionOp::Expr(value) => {
                self.collect_expr(index, value);
            }
            FunctionOp::MemoryIntrinsic(memory) => {
                self.collect_expr(index, &memory.dest);
                match &memory.source {
                    FunctionMemoryIntrinsicSource::Slice(expr)
                    | FunctionMemoryIntrinsicSource::Byte(expr) => {
                        self.collect_expr(index, expr);
                    }
                }
            }
            FunctionOp::Defer(body) => self.collect_defer_body(index, body),
        }
    }

    fn collect_terminator(&mut self, index: &ProgramIndex<'_>, terminator: &FunctionTerminator) {
        match terminator {
            FunctionTerminator::If { cond, .. } => self.collect_expr(index, cond),
            FunctionTerminator::Switch { target, arms, .. } => {
                self.collect_expr(index, target);
                for arm in arms {
                    self.collect_expr(index, &arm.pattern);
                }
            }
            FunctionTerminator::Try { value, .. } => self.collect_expr(index, value),
            FunctionTerminator::Loop { header, .. } => match header {
                FunctionForHeader::Infinite => {}
                FunctionForHeader::Condition(expr) => self.collect_expr(index, expr),
            },
            FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
                if let Some(value) = value {
                    self.collect_expr(index, value);
                }
            }
            FunctionTerminator::Error { .. }
            | FunctionTerminator::Branch { .. }
            | FunctionTerminator::Next { .. } => {}
        }
    }

    fn collect_expr(&mut self, index: &ProgramIndex<'_>, expr: &FunctionExpr) {
        match &expr.kind {
            FunctionExprKind::Binary { lhs, op, rhs } => {
                self.collect_binary(index, lhs, *op);
                self.collect_expr(index, lhs);
                self.collect_expr(index, rhs);
            }
            FunctionExprKind::Assign { place, op, rhs } => {
                self.collect_assign(index, place, *op);
                self.collect_place(index, place);
                self.collect_expr(index, rhs);
            }
            FunctionExprKind::Range(range) => {
                if let Some(start) = &range.start {
                    self.collect_expr(index, start);
                }
                if let Some(end) = &range.end {
                    self.collect_expr(index, end);
                }
            }
            FunctionExprKind::RangeBound { range, .. } => self.collect_expr(index, range),
            FunctionExprKind::InlineAsm(asm) => {
                for input in &asm.inputs {
                    self.collect_expr(index, &input.value);
                }
                for output in &asm.outputs {
                    self.collect_place(index, &output.place);
                }
            }
            FunctionExprKind::Atomic(atomic) => self.collect_atomic(index, atomic),
            FunctionExprKind::StaticArrayPointer { array, .. } => self.collect_expr(index, array),
            FunctionExprKind::ArrayLiteral { elems } => match elems {
                FunctionArrayElements::List(elems) => {
                    for elem in elems {
                        self.collect_expr(index, elem);
                    }
                }
                FunctionArrayElements::Repeat { value, .. } => self.collect_expr(index, value),
            },
            FunctionExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    self.collect_expr(index, &field.value);
                }
            }
            FunctionExprKind::UnionLiteral { field, .. } => {
                self.collect_expr(index, &field.value);
            }
            FunctionExprKind::Unary { expr, .. }
            | FunctionExprKind::OptionalSome { expr }
            | FunctionExprKind::ErrorOk { expr }
            | FunctionExprKind::ErrorErr { expr }
            | FunctionExprKind::TaggedUnionTag { expr }
            | FunctionExprKind::TaggedUnionPayload { expr }
            | FunctionExprKind::Try { expr }
            | FunctionExprKind::LoadUnaligned { ptr: expr, .. }
            | FunctionExprKind::Splat { value: expr }
            | FunctionExprKind::Bitmask { vector: expr }
            | FunctionExprKind::BitIntrinsic { value: expr, .. }
            | FunctionExprKind::CharFromU32 { value: expr }
            | FunctionExprKind::Discard(expr)
            | FunctionExprKind::Cast { expr, .. }
            | FunctionExprKind::TraitObjectUpcast { expr, .. }
            | FunctionExprKind::TraitObjectCoercion { expr, .. } => self.collect_expr(index, expr),
            FunctionExprKind::AddrOf(place) => self.collect_place(index, place),
            FunctionExprKind::ExtractElement {
                vector,
                index: element_index,
            } => {
                self.collect_expr(index, vector);
                self.collect_expr(index, element_index);
            }
            FunctionExprKind::InsertElement {
                vector,
                index: element_index,
                value,
            } => {
                self.collect_expr(index, vector);
                self.collect_expr(index, element_index);
                self.collect_expr(index, value);
            }
            FunctionExprKind::Call { callee, args } => {
                self.collect_callee(index, callee, args);
                for arg in args {
                    self.collect_expr(index, arg);
                }
            }
            FunctionExprKind::Field { lhs, .. } => self.collect_expr(index, lhs),
            FunctionExprKind::Index {
                lhs,
                index: element_index,
            } => {
                self.collect_expr(index, lhs);
                self.collect_expr(index, element_index);
            }
            FunctionExprKind::Slice { lhs, range, .. } => {
                self.collect_expr(index, lhs);
                self.collect_slice_range(index, range);
            }
            FunctionExprKind::Error
            | FunctionExprKind::Integer(_)
            | FunctionExprKind::Float(_)
            | FunctionExprKind::String(_)
            | FunctionExprKind::ByteString(_)
            | FunctionExprKind::Char(_)
            | FunctionExprKind::ByteChar(_)
            | FunctionExprKind::Bool(_)
            | FunctionExprKind::Null
            | FunctionExprKind::ConstGeneric(_)
            | FunctionExprKind::Local(_)
            | FunctionExprKind::Global(_)
            | FunctionExprKind::GlobalInstance { .. }
            | FunctionExprKind::Function(_)
            | FunctionExprKind::FunctionInstance { .. }
            | FunctionExprKind::EnumVariant(_)
            | FunctionExprKind::BuiltinValue(_)
            | FunctionExprKind::Trap => {}
        }
    }

    fn collect_binary(&mut self, index: &ProgramIndex<'_>, lhs: &FunctionExpr, op: BinaryOp) {
        if !matches!(op, BinaryOp::Div | BinaryOp::Rem) {
            return;
        }
        match index.ty_kind(lhs.ty) {
            Some(TyKind::Primitive(PrimitiveTy::U128)) => self.symbols.u128_div_rem = true,
            Some(TyKind::Primitive(PrimitiveTy::I128)) => self.symbols.i128_div_rem = true,
            _ => {}
        }
    }

    fn collect_assign(&mut self, index: &ProgramIndex<'_>, place: &FunctionPlace, op: AssignOp) {
        if !matches!(op, AssignOp::Div | AssignOp::Rem) {
            return;
        }
        match index.ty_kind(place.ty) {
            Some(TyKind::Primitive(PrimitiveTy::U128)) => self.symbols.u128_div_rem = true,
            Some(TyKind::Primitive(PrimitiveTy::I128)) => self.symbols.i128_div_rem = true,
            _ => {}
        }
    }

    fn collect_atomic(
        &mut self,
        index: &ProgramIndex<'_>,
        atomic: &nia_function_ir::FunctionAtomic,
    ) {
        match atomic {
            nia_function_ir::FunctionAtomic::Load { ptr, .. } => self.collect_expr(index, ptr),
            nia_function_ir::FunctionAtomic::Store { ptr, value, .. }
            | nia_function_ir::FunctionAtomic::Rmw { ptr, value, .. } => {
                self.collect_expr(index, ptr);
                self.collect_expr(index, value);
            }
            nia_function_ir::FunctionAtomic::Cmpxchg {
                ptr,
                expected,
                desired,
                ..
            } => {
                self.collect_expr(index, ptr);
                self.collect_expr(index, expected);
                self.collect_expr(index, desired);
            }
            nia_function_ir::FunctionAtomic::Fence { .. } => {}
        }
    }

    fn collect_callee(
        &mut self,
        index: &ProgramIndex<'_>,
        callee: &FunctionCallee,
        args: &[FunctionExpr],
    ) {
        match callee {
            FunctionCallee::BuiltinOperator(operator) => {
                if let FunctionBuiltinOperatorOp::Binary(op) = operator.op
                    && let Some(lhs) = args.first()
                {
                    self.collect_binary(index, lhs, op);
                }
            }
            FunctionCallee::Method { receiver, .. }
            | FunctionCallee::TraitMethod { receiver, .. }
            | FunctionCallee::DynamicTraitMethod { receiver, .. }
            | FunctionCallee::BuiltinMethod { receiver, .. }
            | FunctionCallee::BuiltinPlaceMethod { receiver, .. }
            | FunctionCallee::FunctionPointer(receiver) => self.collect_expr(index, receiver),
            FunctionCallee::Function(_)
            | FunctionCallee::FunctionInstance { .. }
            | FunctionCallee::TraitAssociatedFunction { .. } => {}
        }
    }

    fn collect_place(&mut self, index: &ProgramIndex<'_>, place: &FunctionPlace) {
        match &place.base {
            FunctionPlaceBase::Deref(expr) => self.collect_expr(index, expr),
            FunctionPlaceBase::Local(_)
            | FunctionPlaceBase::Global(_)
            | FunctionPlaceBase::GlobalInstance { .. }
            | FunctionPlaceBase::Error => {}
        }
        for elem in &place.elems {
            match elem {
                FunctionPlaceElem::Index(expr) => self.collect_expr(index, expr),
                FunctionPlaceElem::Field(_) | FunctionPlaceElem::Error => {}
            }
        }
    }

    fn collect_slice_range(&mut self, index: &ProgramIndex<'_>, range: &FunctionSliceRange) {
        if let Some(start) = &range.start {
            self.collect_expr(index, start);
        }
        if let Some(end) = &range.end {
            self.collect_expr(index, end);
        }
    }
}

pub(crate) fn emit_object(
    target: &TargetMachine,
    symbols: CompilerBuiltinSymbols,
) -> Result<Vec<u8>, Diagnostic> {
    let context = Context::create();
    let module = context
        .create_module("nia.compiler_builtins")
        .map_err(diagnostic_from_llvm_error)?;
    target
        .configure_module(&module)
        .map_err(|error| error.diagnostic())?;
    if symbols.u128_div_rem {
        emit_u128_div_rem(&context, &module, false)?;
    }
    if symbols.i128_div_rem {
        emit_u128_div_rem(&context, &module, true)?;
    }
    module.verify().map_err(diagnostic_from_llvm_error)?;
    target
        .emit_object(&module)
        .map_err(diagnostic_from_llvm_error)
}

fn emit_u128_div_rem<'ctx>(
    context: &'ctx Context,
    module: &nia_llvm::module::Module<'ctx>,
    signed: bool,
) -> Result<(), Diagnostic> {
    let i128_ty = context.i128_type();
    let fn_ty = i128_ty.fn_type(&[i128_ty.into(), i128_ty.into()], false);
    let div = module
        .add_function(
            if signed { "__divti3" } else { "__udivti3" },
            fn_ty,
            Some(Linkage::External),
        )
        .map_err(diagnostic_from_llvm_error)?;
    let rem = module
        .add_function(
            if signed { "__modti3" } else { "__umodti3" },
            fn_ty,
            Some(Linkage::External),
        )
        .map_err(diagnostic_from_llvm_error)?;

    let divmod_ty = i128_ty.fn_type(
        &[i128_ty.into(), i128_ty.into(), context.bool_type().into()],
        false,
    );
    let divmod = module
        .add_function(
            if signed {
                "__nia_sdivmodti4"
            } else {
                "__nia_udivmodti4"
            },
            divmod_ty,
            Some(Linkage::Internal),
        )
        .map_err(diagnostic_from_llvm_error)?;

    let builder = context.create_builder();
    let a = divmod
        .get_nth_param(0)
        .ok_or_else(|| diagnostic_from_llvm_error(LlvmError::ice("missing builtin param")))?
        .map_err(diagnostic_from_llvm_error)?
        .into_int_value()
        .map_err(diagnostic_from_llvm_error)?;
    let b = divmod
        .get_nth_param(1)
        .ok_or_else(|| diagnostic_from_llvm_error(LlvmError::ice("missing builtin param")))?
        .map_err(diagnostic_from_llvm_error)?
        .into_int_value()
        .map_err(diagnostic_from_llvm_error)?;
    let want_rem = divmod
        .get_nth_param(2)
        .ok_or_else(|| diagnostic_from_llvm_error(LlvmError::ice("missing builtin param")))?
        .map_err(diagnostic_from_llvm_error)?
        .into_int_value()
        .map_err(diagnostic_from_llvm_error)?;

    let entry = context
        .append_basic_block(divmod, "entry")
        .map_err(diagnostic_from_llvm_error)?;
    let loop_block = context
        .append_basic_block(divmod, "loop")
        .map_err(diagnostic_from_llvm_error)?;
    let body_block = context
        .append_basic_block(divmod, "body")
        .map_err(diagnostic_from_llvm_error)?;
    let sub_block = context
        .append_basic_block(divmod, "sub")
        .map_err(diagnostic_from_llvm_error)?;
    let cont_block = context
        .append_basic_block(divmod, "cont")
        .map_err(diagnostic_from_llvm_error)?;
    let done_block = context
        .append_basic_block(divmod, "done")
        .map_err(diagnostic_from_llvm_error)?;
    let trap_block = context
        .append_basic_block(divmod, "trap")
        .map_err(diagnostic_from_llvm_error)?;

    builder.position_at_end(entry);
    let quotient = builder
        .build_alloca(i128_ty, "quotient")
        .map_err(diagnostic_from_llvm_error)?;
    let remainder = builder
        .build_alloca(i128_ty, "remainder")
        .map_err(diagnostic_from_llvm_error)?;
    let shift = builder
        .build_alloca(i128_ty, "shift")
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_store(quotient, i128_ty.const_zero())
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_store(remainder, i128_ty.const_zero())
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_store(shift, i128_ty.const_int(128, false))
        .map_err(diagnostic_from_llvm_error)?;
    let div_by_zero = builder
        .build_int_compare(IntPredicate::EQ, b, i128_ty.const_zero(), "divzero")
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_conditional_branch(div_by_zero, trap_block, loop_block)
        .map_err(diagnostic_from_llvm_error)?;

    builder.position_at_end(trap_block);
    builder
        .build_unreachable()
        .map_err(diagnostic_from_llvm_error)?;

    builder.position_at_end(loop_block);
    let current_shift = load_i128(&builder, i128_ty, shift, "shift.load")?;
    let keep_going = builder
        .build_int_compare(
            IntPredicate::UGT,
            current_shift,
            i128_ty.const_zero(),
            "keepgoing",
        )
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_conditional_branch(keep_going, body_block, done_block)
        .map_err(diagnostic_from_llvm_error)?;

    builder.position_at_end(body_block);
    let next_shift = builder
        .build_int_sub(current_shift, i128_ty.const_int(1, false), "nextshift")
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_store(shift, next_shift)
        .map_err(diagnostic_from_llvm_error)?;
    let rem_value = load_i128(&builder, i128_ty, remainder, "rem.load")?;
    let rem_shifted = builder
        .build_left_shift(rem_value, i128_ty.const_int(1, false), "rem.shift")
        .map_err(diagnostic_from_llvm_error)?;
    let bit = builder
        .build_right_shift(a, next_shift, false, "bit.shift")
        .and_then(|value| builder.build_and(value, i128_ty.const_int(1, false), "bit"))
        .map_err(diagnostic_from_llvm_error)?;
    let rem_next = builder
        .build_or(rem_shifted, bit, "rem.next")
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_store(remainder, rem_next)
        .map_err(diagnostic_from_llvm_error)?;
    let can_sub = builder
        .build_int_compare(IntPredicate::UGE, rem_next, b, "cansub")
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_conditional_branch(can_sub, sub_block, cont_block)
        .map_err(diagnostic_from_llvm_error)?;

    builder.position_at_end(sub_block);
    let rem_sub = builder
        .build_int_sub(rem_next, b, "rem.sub")
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_store(remainder, rem_sub)
        .map_err(diagnostic_from_llvm_error)?;
    let quotient_value = load_i128(&builder, i128_ty, quotient, "quo.load")?;
    let quotient_bit = builder
        .build_left_shift(i128_ty.const_int(1, false), next_shift, "quo.bit")
        .map_err(diagnostic_from_llvm_error)?;
    let quotient_next = builder
        .build_or(quotient_value, quotient_bit, "quo.next")
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_store(quotient, quotient_next)
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_unconditional_branch(cont_block)
        .map_err(diagnostic_from_llvm_error)?;

    builder.position_at_end(cont_block);
    builder
        .build_unconditional_branch(loop_block)
        .map_err(diagnostic_from_llvm_error)?;

    builder.position_at_end(done_block);
    let final_quotient = load_i128(&builder, i128_ty, quotient, "final.quo")?;
    let final_remainder = load_i128(&builder, i128_ty, remainder, "final.rem")?;
    let result = builder
        .build_select(
            want_rem.into(),
            final_remainder.into(),
            final_quotient.into(),
            "result",
        )
        .map_err(diagnostic_from_llvm_error)?
        .into_int_value()
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_return(Some(&result))
        .map_err(diagnostic_from_llvm_error)?;

    if signed {
        emit_i128_builtin_wrapper(context, div, divmod, false)?;
        emit_i128_builtin_wrapper(context, rem, divmod, true)?;
    } else {
        emit_u128_builtin_wrapper(context, div, divmod, false)?;
        emit_u128_builtin_wrapper(context, rem, divmod, true)?;
    }
    Ok(())
}

fn emit_u128_builtin_wrapper<'ctx>(
    context: &'ctx Context,
    function: nia_llvm::values::FunctionValue<'ctx>,
    divmod: nia_llvm::values::FunctionValue<'ctx>,
    want_rem: bool,
) -> Result<(), Diagnostic> {
    let i1_ty = context.bool_type();
    let entry = context
        .append_basic_block(function, "entry")
        .map_err(diagnostic_from_llvm_error)?;
    let builder = context.create_builder();
    builder.position_at_end(entry);
    let a = function
        .get_nth_param(0)
        .ok_or_else(|| diagnostic_from_llvm_error(LlvmError::ice("missing builtin param")))?
        .map_err(diagnostic_from_llvm_error)?
        .into_int_value()
        .map_err(diagnostic_from_llvm_error)?;
    let b = function
        .get_nth_param(1)
        .ok_or_else(|| diagnostic_from_llvm_error(LlvmError::ice("missing builtin param")))?
        .map_err(diagnostic_from_llvm_error)?
        .into_int_value()
        .map_err(diagnostic_from_llvm_error)?;
    let args: [BasicMetadataValueEnum<'ctx>; 3] = [
        a.into(),
        b.into(),
        i1_ty.const_int(want_rem as u64, false).into(),
    ];
    let result = builder
        .build_call(divmod, &args, "builtin")
        .map_err(diagnostic_from_llvm_error)?
        .try_as_basic_value()
        .unwrap_basic()
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_return(Some(&result))
        .map_err(diagnostic_from_llvm_error)?;
    Ok(())
}

fn emit_i128_builtin_wrapper<'ctx>(
    context: &'ctx Context,
    function: nia_llvm::values::FunctionValue<'ctx>,
    divmod: nia_llvm::values::FunctionValue<'ctx>,
    want_rem: bool,
) -> Result<(), Diagnostic> {
    let i128_ty = context.i128_type();
    let i1_ty = context.bool_type();
    let entry = context
        .append_basic_block(function, "entry")
        .map_err(diagnostic_from_llvm_error)?;
    let builder = context.create_builder();
    builder.position_at_end(entry);
    let a = function
        .get_nth_param(0)
        .ok_or_else(|| diagnostic_from_llvm_error(LlvmError::ice("missing builtin param")))?
        .map_err(diagnostic_from_llvm_error)?
        .into_int_value()
        .map_err(diagnostic_from_llvm_error)?;
    let b = function
        .get_nth_param(1)
        .ok_or_else(|| diagnostic_from_llvm_error(LlvmError::ice("missing builtin param")))?
        .map_err(diagnostic_from_llvm_error)?
        .into_int_value()
        .map_err(diagnostic_from_llvm_error)?;
    let zero = i128_ty.const_zero();
    let a_neg = builder
        .build_int_compare(IntPredicate::SLT, a, zero, "a.neg")
        .map_err(diagnostic_from_llvm_error)?;
    let b_neg = builder
        .build_int_compare(IntPredicate::SLT, b, zero, "b.neg")
        .map_err(diagnostic_from_llvm_error)?;
    let neg_a = builder
        .build_int_neg(a, "neg.a")
        .map_err(diagnostic_from_llvm_error)?;
    let neg_b = builder
        .build_int_neg(b, "neg.b")
        .map_err(diagnostic_from_llvm_error)?;
    let abs_a = builder
        .build_select(a_neg.into(), neg_a.into(), a.into(), "abs.a")
        .map_err(diagnostic_from_llvm_error)?
        .into_int_value()
        .map_err(diagnostic_from_llvm_error)?;
    let abs_b = builder
        .build_select(b_neg.into(), neg_b.into(), b.into(), "abs.b")
        .map_err(diagnostic_from_llvm_error)?
        .into_int_value()
        .map_err(diagnostic_from_llvm_error)?;
    let args: [BasicMetadataValueEnum<'ctx>; 3] = [
        abs_a.into(),
        abs_b.into(),
        i1_ty.const_int(want_rem as u64, false).into(),
    ];
    let unsigned = builder
        .build_call(divmod, &args, "unsigned")
        .map_err(diagnostic_from_llvm_error)?
        .try_as_basic_value()
        .unwrap_basic()
        .map_err(diagnostic_from_llvm_error)?
        .into_int_value()
        .map_err(diagnostic_from_llvm_error)?;
    let result_neg = if want_rem {
        a_neg
    } else {
        builder
            .build_xor(a_neg, b_neg, "result.neg")
            .map_err(diagnostic_from_llvm_error)?
    };
    let neg_unsigned = builder
        .build_int_neg(unsigned, "neg.unsigned")
        .map_err(diagnostic_from_llvm_error)?;
    let result = builder
        .build_select(
            result_neg.into(),
            neg_unsigned.into(),
            unsigned.into(),
            "result",
        )
        .map_err(diagnostic_from_llvm_error)?
        .into_int_value()
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_return(Some(&result))
        .map_err(diagnostic_from_llvm_error)?;
    Ok(())
}

fn load_i128<'ctx>(
    builder: &nia_llvm::builder::Builder<'ctx>,
    ty: nia_llvm::types::IntType<'ctx>,
    ptr: PointerValue<'ctx>,
    name: &str,
) -> Result<IntValue<'ctx>, Diagnostic> {
    builder
        .build_load(ty, ptr, name)
        .and_then(BasicValueEnum::into_int_value)
        .map_err(diagnostic_from_llvm_error)
}

fn diagnostic_from_llvm_error(error: LlvmError) -> Diagnostic {
    error.diagnostic()
}
