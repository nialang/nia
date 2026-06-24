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
use nia_ty::{PrimitiveTy, TyInterner, TyKind};

pub(crate) fn required_symbols(program: &BackendProgram) -> CompilerBuiltinSymbols {
    let mut collector = CompilerBuiltinCollector::default();
    collector.collect_program(program);
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
    fn collect_program(&mut self, program: &BackendProgram) {
        for module in &program.modules {
            for function in &module.functions {
                if let Some(body) = &function.function_body {
                    self.collect_body(&module.interner, body);
                }
            }
            for function in &module.function_instances {
                if let Some(body) = &function.function_body {
                    self.collect_body(&module.interner, body);
                }
            }
        }
    }

    fn collect_body(&mut self, interner: &TyInterner, body: &FunctionBody) {
        for block in &body.blocks {
            for op in &block.ops {
                self.collect_op(interner, op);
            }
            self.collect_terminator(interner, &block.terminator);
        }
    }

    fn collect_defer_body(&mut self, interner: &TyInterner, body: &FunctionDeferBody) {
        for block in &body.blocks {
            for op in &block.ops {
                self.collect_op(interner, op);
            }
            self.collect_terminator(interner, &block.terminator);
        }
    }

    fn collect_op(&mut self, interner: &TyInterner, op: &FunctionOp) {
        match op {
            FunctionOp::Binding(binding) => {
                if let Some(value) = &binding.value {
                    self.collect_expr(interner, value);
                }
            }
            FunctionOp::StoreLocal { value, .. } | FunctionOp::Expr(value) => {
                self.collect_expr(interner, value);
            }
            FunctionOp::MemoryIntrinsic(memory) => {
                self.collect_expr(interner, &memory.dest);
                match &memory.source {
                    FunctionMemoryIntrinsicSource::Slice(expr)
                    | FunctionMemoryIntrinsicSource::Byte(expr) => {
                        self.collect_expr(interner, expr);
                    }
                }
            }
            FunctionOp::Defer(body) => self.collect_defer_body(interner, body),
        }
    }

    fn collect_terminator(&mut self, interner: &TyInterner, terminator: &FunctionTerminator) {
        match terminator {
            FunctionTerminator::If { cond, .. } => self.collect_expr(interner, cond),
            FunctionTerminator::Switch { target, arms, .. } => {
                self.collect_expr(interner, target);
                for arm in arms {
                    self.collect_expr(interner, &arm.pattern);
                }
            }
            FunctionTerminator::Try { value, .. } => self.collect_expr(interner, value),
            FunctionTerminator::Loop { header, .. } => match header {
                FunctionForHeader::Infinite => {}
                FunctionForHeader::Condition(expr) => self.collect_expr(interner, expr),
            },
            FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
                if let Some(value) = value {
                    self.collect_expr(interner, value);
                }
            }
            FunctionTerminator::Error { .. }
            | FunctionTerminator::Branch { .. }
            | FunctionTerminator::Next { .. } => {}
        }
    }

    fn collect_expr(&mut self, interner: &TyInterner, expr: &FunctionExpr) {
        match &expr.kind {
            FunctionExprKind::Binary { lhs, op, rhs } => {
                self.collect_binary(interner, lhs, *op);
                self.collect_expr(interner, lhs);
                self.collect_expr(interner, rhs);
            }
            FunctionExprKind::Assign { place, op, rhs } => {
                self.collect_assign(interner, place, *op);
                self.collect_place(interner, place);
                self.collect_expr(interner, rhs);
            }
            FunctionExprKind::Range(range) => {
                if let Some(start) = &range.start {
                    self.collect_expr(interner, start);
                }
                if let Some(end) = &range.end {
                    self.collect_expr(interner, end);
                }
            }
            FunctionExprKind::RangeBound { range, .. } => self.collect_expr(interner, range),
            FunctionExprKind::InlineAsm(asm) => {
                for input in &asm.inputs {
                    self.collect_expr(interner, &input.value);
                }
                for output in &asm.outputs {
                    self.collect_place(interner, &output.place);
                }
            }
            FunctionExprKind::Atomic(atomic) => self.collect_atomic(interner, atomic),
            FunctionExprKind::StaticArrayPointer { array, .. } => {
                self.collect_expr(interner, array)
            }
            FunctionExprKind::ArrayLiteral { elems } => match elems {
                FunctionArrayElements::List(elems) => {
                    for elem in elems {
                        self.collect_expr(interner, elem);
                    }
                }
                FunctionArrayElements::Repeat { value, .. } => self.collect_expr(interner, value),
            },
            FunctionExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    self.collect_expr(interner, &field.value);
                }
            }
            FunctionExprKind::UnionLiteral { field, .. } => {
                self.collect_expr(interner, &field.value);
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
            | FunctionExprKind::TraitObjectCoercion { expr, .. } => {
                self.collect_expr(interner, expr)
            }
            FunctionExprKind::AddrOf(place) => self.collect_place(interner, place),
            FunctionExprKind::ExtractElement { vector, index } => {
                self.collect_expr(interner, vector);
                self.collect_expr(interner, index);
            }
            FunctionExprKind::InsertElement {
                vector,
                index,
                value,
            } => {
                self.collect_expr(interner, vector);
                self.collect_expr(interner, index);
                self.collect_expr(interner, value);
            }
            FunctionExprKind::Call { callee, args } => {
                self.collect_callee(interner, callee, args);
                for arg in args {
                    self.collect_expr(interner, arg);
                }
            }
            FunctionExprKind::Field { lhs, .. } => self.collect_expr(interner, lhs),
            FunctionExprKind::Index { lhs, index } => {
                self.collect_expr(interner, lhs);
                self.collect_expr(interner, index);
            }
            FunctionExprKind::Slice { lhs, range, .. } => {
                self.collect_expr(interner, lhs);
                self.collect_slice_range(interner, range);
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
            | FunctionExprKind::Local(_)
            | FunctionExprKind::Global(_)
            | FunctionExprKind::Function(_)
            | FunctionExprKind::FunctionInstance { .. }
            | FunctionExprKind::EnumVariant(_)
            | FunctionExprKind::BuiltinValue(_)
            | FunctionExprKind::Trap => {}
        }
    }

    fn collect_binary(&mut self, interner: &TyInterner, lhs: &FunctionExpr, op: BinaryOp) {
        if !matches!(op, BinaryOp::Div | BinaryOp::Rem) {
            return;
        }
        match interner.get(lhs.ty) {
            Some(TyKind::Primitive(PrimitiveTy::U128)) => self.symbols.u128_div_rem = true,
            Some(TyKind::Primitive(PrimitiveTy::I128)) => self.symbols.i128_div_rem = true,
            _ => {}
        }
    }

    fn collect_assign(&mut self, interner: &TyInterner, place: &FunctionPlace, op: AssignOp) {
        if !matches!(op, AssignOp::Div | AssignOp::Rem) {
            return;
        }
        match interner.get(place.ty) {
            Some(TyKind::Primitive(PrimitiveTy::U128)) => self.symbols.u128_div_rem = true,
            Some(TyKind::Primitive(PrimitiveTy::I128)) => self.symbols.i128_div_rem = true,
            _ => {}
        }
    }

    fn collect_atomic(&mut self, interner: &TyInterner, atomic: &nia_function_ir::FunctionAtomic) {
        match atomic {
            nia_function_ir::FunctionAtomic::Load { ptr, .. } => self.collect_expr(interner, ptr),
            nia_function_ir::FunctionAtomic::Store { ptr, value, .. }
            | nia_function_ir::FunctionAtomic::Rmw { ptr, value, .. } => {
                self.collect_expr(interner, ptr);
                self.collect_expr(interner, value);
            }
            nia_function_ir::FunctionAtomic::Cmpxchg {
                ptr,
                expected,
                desired,
                ..
            } => {
                self.collect_expr(interner, ptr);
                self.collect_expr(interner, expected);
                self.collect_expr(interner, desired);
            }
            nia_function_ir::FunctionAtomic::Fence { .. } => {}
        }
    }

    fn collect_callee(
        &mut self,
        interner: &TyInterner,
        callee: &FunctionCallee,
        args: &[FunctionExpr],
    ) {
        match callee {
            FunctionCallee::BuiltinOperator(operator) => {
                if let FunctionBuiltinOperatorOp::Binary(op) = operator.op
                    && let Some(lhs) = args.first()
                {
                    self.collect_binary(interner, lhs, op);
                }
            }
            FunctionCallee::Method { receiver, .. }
            | FunctionCallee::TraitMethod { receiver, .. }
            | FunctionCallee::DynamicTraitMethod { receiver, .. }
            | FunctionCallee::BuiltinMethod { receiver, .. }
            | FunctionCallee::BuiltinPlaceMethod { receiver, .. }
            | FunctionCallee::FunctionPointer(receiver) => self.collect_expr(interner, receiver),
            FunctionCallee::Function(_)
            | FunctionCallee::FunctionInstance { .. }
            | FunctionCallee::TraitAssociatedFunction { .. } => {}
        }
    }

    fn collect_place(&mut self, interner: &TyInterner, place: &FunctionPlace) {
        match &place.base {
            FunctionPlaceBase::Deref(expr) => self.collect_expr(interner, expr),
            FunctionPlaceBase::Local(_)
            | FunctionPlaceBase::Global(_)
            | FunctionPlaceBase::Error => {}
        }
        for elem in &place.elems {
            match elem {
                FunctionPlaceElem::Index(expr) => self.collect_expr(interner, expr),
                FunctionPlaceElem::Field(_) | FunctionPlaceElem::Error => {}
            }
        }
    }

    fn collect_slice_range(&mut self, interner: &TyInterner, range: &FunctionSliceRange) {
        if let Some(start) = &range.start {
            self.collect_expr(interner, start);
        }
        if let Some(end) = &range.end {
            self.collect_expr(interner, end);
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
