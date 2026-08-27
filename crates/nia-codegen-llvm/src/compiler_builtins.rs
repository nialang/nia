// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{AssignOp, BinaryOp};
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    FunctionArrayElements, FunctionBody, FunctionBuiltinOperatorOp, FunctionCallee,
    FunctionDeferBody, FunctionExpr, FunctionExprKind, FunctionForHeader,
    FunctionMemoryIntrinsicSource, FunctionOp, FunctionPlace, FunctionPlaceBase, FunctionPlaceElem,
    FunctionSliceRange, FunctionTerminator,
};
use nia_llvm::{
    Context, FloatPredicate, IntPredicate, LlvmError,
    module::Linkage,
    target::TargetMachine,
    values::{BasicMetadataValueEnum, BasicValueEnum, IntValue, PointerValue},
};
use nia_ty::{PrimitiveTy, TyKind};

use crate::program_index::ProgramIndex;

pub(crate) fn required_symbols(index: &ProgramIndex) -> CompilerBuiltinSymbols {
    let mut collector = CompilerBuiltinCollector::default();
    collector.collect_program(index);
    collector.symbols
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CompilerBuiltinSymbols {
    pub(crate) u128_div_rem: bool,
    pub(crate) i128_div_rem: bool,
    pub(crate) u128_from_f32: bool,
    pub(crate) u128_from_f64: bool,
    pub(crate) i128_from_f32: bool,
    pub(crate) i128_from_f64: bool,
    pub(crate) f32_from_u128: bool,
    pub(crate) f64_from_u128: bool,
    pub(crate) f32_from_i128: bool,
    pub(crate) f64_from_i128: bool,
}

impl CompilerBuiltinSymbols {
    pub(crate) fn any(self) -> bool {
        self.u128_div_rem
            || self.i128_div_rem
            || self.u128_from_f32
            || self.u128_from_f64
            || self.i128_from_f32
            || self.i128_from_f64
            || self.f32_from_u128
            || self.f64_from_u128
            || self.f32_from_i128
            || self.f64_from_i128
    }

    /// Returns the externally visible definitions emitted for this exact set.
    ///
    /// Keep this list beside the emitter flags: native-program validation uses
    /// it to reserve only helpers that the compiler-builtins object will own.
    pub(crate) fn external_definitions(self) -> impl Iterator<Item = &'static str> {
        [
            self.u128_div_rem.then_some("__udivti3"),
            self.u128_div_rem.then_some("__umodti3"),
            self.i128_div_rem.then_some("__divti3"),
            self.i128_div_rem.then_some("__modti3"),
            self.u128_from_f32.then_some("__fixunssfti"),
            self.u128_from_f64.then_some("__fixunsdfti"),
            self.i128_from_f32.then_some("__fixsfti"),
            self.i128_from_f64.then_some("__fixdfti"),
            self.f32_from_u128.then_some("__floatuntisf"),
            self.f64_from_u128.then_some("__floatuntidf"),
            self.f32_from_i128.then_some("__floattisf"),
            self.f64_from_i128.then_some("__floattidf"),
        ]
        .into_iter()
        .flatten()
    }
}

#[derive(Debug, Default)]
struct CompilerBuiltinCollector {
    symbols: CompilerBuiltinSymbols,
}

impl CompilerBuiltinCollector {
    fn collect_program(&mut self, index: &ProgramIndex) {
        for module_id in index.module_ids() {
            let module = index
                .module(*module_id)
                .expect("compiler builtin scan requires a published backend module");
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

    fn collect_body(&mut self, index: &ProgramIndex, body: &FunctionBody) {
        for block in &body.blocks {
            for op in &block.ops {
                self.collect_op(index, op);
            }
            self.collect_terminator(index, &block.terminator);
        }
    }

    fn collect_defer_body(&mut self, index: &ProgramIndex, body: &FunctionDeferBody) {
        for block in &body.blocks {
            for op in &block.ops {
                self.collect_op(index, op);
            }
            self.collect_terminator(index, &block.terminator);
        }
    }

    fn collect_op(&mut self, index: &ProgramIndex, op: &FunctionOp) {
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

    fn collect_terminator(&mut self, index: &ProgramIndex, terminator: &FunctionTerminator) {
        match terminator {
            FunctionTerminator::If { cond, .. } => self.collect_expr(index, cond),
            FunctionTerminator::Switch { target, arms, .. } => {
                self.collect_expr(index, target);
                for arm in arms {
                    self.collect_expr(index, &arm.pattern);
                }
            }
            FunctionTerminator::Try {
                value,
                error_conversion,
                ..
            } => {
                self.collect_expr(index, value);
                if let Some(conversion) = error_conversion {
                    self.collect_expr(index, conversion);
                }
            }
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

    fn collect_expr(&mut self, index: &ProgramIndex, expr: &FunctionExpr) {
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
            FunctionExprKind::Tuple(elems) => {
                for elem in elems {
                    self.collect_expr(index, elem);
                }
            }
            FunctionExprKind::TupleField { value, .. } => self.collect_expr(index, value),
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
            | FunctionExprKind::TraitObjectUpcast { expr, .. }
            | FunctionExprKind::TraitObjectCoercion { expr, .. }
            | FunctionExprKind::CallableCoercion { state: expr, .. } => {
                self.collect_expr(index, expr)
            }
            FunctionExprKind::Cast {
                expr: inner,
                ty: target_ty,
            } => {
                self.collect_cast(index, inner.ty, *target_ty);
                self.collect_expr(index, inner);
            }
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
            FunctionExprKind::EnumVariant { fields, .. } => {
                for field in fields {
                    self.collect_expr(index, field);
                }
            }
            FunctionExprKind::EnumTag { value }
            | FunctionExprKind::EnumPayloadField { value, .. } => self.collect_expr(index, value),
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
            | FunctionExprKind::ClosureFunctionPointer { .. }
            | FunctionExprKind::EnumVariantTag(_)
            | FunctionExprKind::BuiltinValue(_)
            | FunctionExprKind::Trap => {}
            FunctionExprKind::UnionStorageLiteral { relocations, .. } => {
                for relocation in relocations {
                    self.collect_expr(index, &relocation.pointee);
                }
            }
        }
    }

    fn collect_binary(&mut self, index: &ProgramIndex, lhs: &FunctionExpr, op: BinaryOp) {
        if !matches!(op, BinaryOp::Div | BinaryOp::Rem) {
            return;
        }
        match index.ty_kind(lhs.ty) {
            Some(TyKind::Primitive(PrimitiveTy::U128)) => self.symbols.u128_div_rem = true,
            Some(TyKind::Primitive(PrimitiveTy::I128)) => self.symbols.i128_div_rem = true,
            _ => {}
        }
    }

    fn collect_assign(&mut self, index: &ProgramIndex, place: &FunctionPlace, op: AssignOp) {
        if !matches!(op, AssignOp::Div | AssignOp::Rem) {
            return;
        }
        match index.ty_kind(place.ty) {
            Some(TyKind::Primitive(PrimitiveTy::U128)) => self.symbols.u128_div_rem = true,
            Some(TyKind::Primitive(PrimitiveTy::I128)) => self.symbols.i128_div_rem = true,
            _ => {}
        }
    }

    fn collect_cast(
        &mut self,
        index: &ProgramIndex,
        source_ty: nia_ids::InternedTyId,
        target_ty: nia_ids::InternedTyId,
    ) {
        match (index.ty_kind(source_ty), index.ty_kind(target_ty)) {
            (
                Some(TyKind::Primitive(PrimitiveTy::F32)),
                Some(TyKind::Primitive(PrimitiveTy::U128)),
            ) => self.symbols.u128_from_f32 = true,
            (
                Some(TyKind::Primitive(PrimitiveTy::F64)),
                Some(TyKind::Primitive(PrimitiveTy::U128)),
            ) => self.symbols.u128_from_f64 = true,
            (
                Some(TyKind::Primitive(PrimitiveTy::F32)),
                Some(TyKind::Primitive(PrimitiveTy::I128)),
            ) => self.symbols.i128_from_f32 = true,
            (
                Some(TyKind::Primitive(PrimitiveTy::F64)),
                Some(TyKind::Primitive(PrimitiveTy::I128)),
            ) => self.symbols.i128_from_f64 = true,
            (
                Some(TyKind::Primitive(PrimitiveTy::U128)),
                Some(TyKind::Primitive(PrimitiveTy::F32)),
            ) => self.symbols.f32_from_u128 = true,
            (
                Some(TyKind::Primitive(PrimitiveTy::U128)),
                Some(TyKind::Primitive(PrimitiveTy::F64)),
            ) => self.symbols.f64_from_u128 = true,
            (
                Some(TyKind::Primitive(PrimitiveTy::I128)),
                Some(TyKind::Primitive(PrimitiveTy::F32)),
            ) => self.symbols.f32_from_i128 = true,
            (
                Some(TyKind::Primitive(PrimitiveTy::I128)),
                Some(TyKind::Primitive(PrimitiveTy::F64)),
            ) => self.symbols.f64_from_i128 = true,
            _ => {}
        }
    }

    fn collect_atomic(&mut self, index: &ProgramIndex, atomic: &nia_function_ir::FunctionAtomic) {
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
        index: &ProgramIndex,
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
            FunctionCallee::ClosureEntry {
                state: receiver, ..
            }
            | FunctionCallee::Method { receiver, .. }
            | FunctionCallee::TraitMethod { receiver, .. }
            | FunctionCallee::DynamicTraitMethod { receiver, .. }
            | FunctionCallee::BuiltinMethod { receiver, .. }
            | FunctionCallee::BuiltinPlaceMethod { receiver, .. }
            | FunctionCallee::Callable(receiver)
            | FunctionCallee::FunctionPointer(receiver) => self.collect_expr(index, receiver),
            FunctionCallee::Function(_)
            | FunctionCallee::FunctionInstance { .. }
            | FunctionCallee::TraitAssociatedFunction { .. } => {}
        }
    }

    fn collect_place(&mut self, index: &ProgramIndex, place: &FunctionPlace) {
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
                FunctionPlaceElem::Field(_)
                | FunctionPlaceElem::TupleField(_)
                | FunctionPlaceElem::Error => {}
            }
        }
    }

    fn collect_slice_range(&mut self, index: &ProgramIndex, range: &FunctionSliceRange) {
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
    let context = Context::create().map_err(diagnostic_from_llvm_error)?;
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
    if symbols.u128_from_f32 {
        emit_i128_from_float(&context, &module, PrimitiveTy::F32, false)?;
    }
    if symbols.u128_from_f64 {
        emit_i128_from_float(&context, &module, PrimitiveTy::F64, false)?;
    }
    if symbols.i128_from_f32 {
        emit_i128_from_float(&context, &module, PrimitiveTy::F32, true)?;
    }
    if symbols.i128_from_f64 {
        emit_i128_from_float(&context, &module, PrimitiveTy::F64, true)?;
    }
    if symbols.f32_from_u128 {
        emit_i128_to_float(&context, &module, PrimitiveTy::F32, false)?;
    }
    if symbols.f64_from_u128 {
        emit_i128_to_float(&context, &module, PrimitiveTy::F64, false)?;
    }
    if symbols.f32_from_i128 {
        emit_i128_to_float(&context, &module, PrimitiveTy::F32, true)?;
    }
    if symbols.f64_from_i128 {
        emit_i128_to_float(&context, &module, PrimitiveTy::F64, true)?;
    }
    module.verify().map_err(diagnostic_from_llvm_error)?;
    target
        .emit_object(&module)
        .map_err(diagnostic_from_llvm_error)
}

fn emit_i128_to_float<'ctx>(
    context: &'ctx Context,
    module: &nia_llvm::module::Module<'ctx>,
    target: PrimitiveTy,
    signed: bool,
) -> Result<(), Diagnostic> {
    let (format, precision) = match target {
        PrimitiveTy::F32 => (
            I128ToFloatFormat {
                target_ty: context.f32_type(),
                storage_ty: context.i32_type(),
                exponent_bias: 127,
                fraction_bits: 23,
                storage_bits: 32,
            },
            24_u32,
        ),
        PrimitiveTy::F64 => (
            I128ToFloatFormat {
                target_ty: context.f64_type(),
                storage_ty: context.i64_type(),
                exponent_bias: 1023,
                fraction_bits: 52,
                storage_bits: 64,
            },
            53_u32,
        ),
        _ => {
            return Err(diagnostic_from_llvm_error(LlvmError::ice(
                "i128 integer conversion builtin requires f32 or f64",
            )));
        }
    };
    let i128_ty = context.i128_type();
    let fn_ty = format
        .target_ty
        .fn_type(&[i128_ty.into()], false)
        .map_err(diagnostic_from_llvm_error)?;
    let function = module
        .add_function(
            match target {
                PrimitiveTy::F32 if signed => "__floattisf",
                PrimitiveTy::F64 if signed => "__floattidf",
                PrimitiveTy::F32 => "__floatuntisf",
                PrimitiveTy::F64 => "__floatuntidf",
                _ => unreachable!("target primitive checked above"),
            },
            fn_ty,
            Some(Linkage::External),
        )
        .map_err(diagnostic_from_llvm_error)?;
    let entry = context
        .append_basic_block(function, "entry")
        .map_err(diagnostic_from_llvm_error)?;
    let zero_block = context
        .append_basic_block(function, "zero")
        .map_err(diagnostic_from_llvm_error)?;
    let classify_block = context
        .append_basic_block(function, "classify")
        .map_err(diagnostic_from_llvm_error)?;
    let exact_block = context
        .append_basic_block(function, "exact")
        .map_err(diagnostic_from_llvm_error)?;
    let round_block = context
        .append_basic_block(function, "round")
        .map_err(diagnostic_from_llvm_error)?;
    let builder = context
        .create_builder()
        .map_err(diagnostic_from_llvm_error)?;
    builder.position_at_end(entry);
    let value = function
        .get_nth_param(0)
        .ok_or_else(|| diagnostic_from_llvm_error(LlvmError::ice("missing builtin param")))?
        .map_err(diagnostic_from_llvm_error)?
        .into_int_value()
        .map_err(diagnostic_from_llvm_error)?;
    let false_value = context
        .bool_type()
        .const_int(0, false)
        .map_err(diagnostic_from_llvm_error)?;
    let (magnitude, is_negative) = if signed {
        let is_negative = builder
            .build_int_compare(
                IntPredicate::SLT,
                value,
                i128_ty.const_zero().map_err(diagnostic_from_llvm_error)?,
                "is_negative",
            )
            .map_err(diagnostic_from_llvm_error)?;
        let negative = builder
            .build_int_neg(value, "magnitude.negative")
            .map_err(diagnostic_from_llvm_error)?;
        let magnitude = builder
            .build_select(
                is_negative.into(),
                negative.into(),
                value.into(),
                "magnitude",
            )
            .and_then(|value| value.into_int_value())
            .map_err(diagnostic_from_llvm_error)?;
        (magnitude, is_negative)
    } else {
        (value, false_value)
    };
    let is_zero = builder
        .build_int_compare(
            IntPredicate::EQ,
            magnitude,
            i128_ty.const_zero().map_err(diagnostic_from_llvm_error)?,
            "is_zero",
        )
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_conditional_branch(is_zero, zero_block, classify_block)
        .map_err(diagnostic_from_llvm_error)?;

    builder.position_at_end(zero_block);
    let zero = format
        .target_ty
        .const_zero()
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_return(Some(&zero))
        .map_err(diagnostic_from_llvm_error)?;

    builder.position_at_end(classify_block);
    let ctlz = nia_llvm::intrinsics::Intrinsic::find("llvm.ctlz")
        .and_then(|intrinsic| intrinsic.get_declaration(module, &[i128_ty.into()]))
        .ok_or_else(|| diagnostic_from_llvm_error(LlvmError::ice("missing ctlz intrinsic")))?;
    let leading = builder
        .build_call(
            ctlz,
            &[magnitude.into(), false_value.into()],
            "leading_zeros",
        )
        .map_err(diagnostic_from_llvm_error)?
        .try_as_basic_value()
        .unwrap_basic()
        .map_err(diagnostic_from_llvm_error)?
        .into_int_value()
        .map_err(diagnostic_from_llvm_error)?;
    let exponent = builder
        .build_int_sub(
            i128_ty
                .const_int(127, false)
                .map_err(diagnostic_from_llvm_error)?,
            leading,
            "exponent",
        )
        .map_err(diagnostic_from_llvm_error)?;
    let requires_rounding = builder
        .build_int_compare(
            IntPredicate::UGE,
            exponent,
            i128_ty
                .const_int(u64::from(precision), false)
                .map_err(diagnostic_from_llvm_error)?,
            "requires_rounding",
        )
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_conditional_branch(requires_rounding, round_block, exact_block)
        .map_err(diagnostic_from_llvm_error)?;

    builder.position_at_end(exact_block);
    let left_shift = builder
        .build_int_sub(
            i128_ty
                .const_int(u64::from(precision - 1), false)
                .map_err(diagnostic_from_llvm_error)?,
            exponent,
            "left_shift",
        )
        .map_err(diagnostic_from_llvm_error)?;
    let significand = builder
        .build_left_shift(magnitude, left_shift, "significand")
        .map_err(diagnostic_from_llvm_error)?;
    emit_i128_to_float_return(
        &builder,
        i128_ty,
        format,
        significand,
        exponent,
        is_negative,
    )?;

    builder.position_at_end(round_block);
    let shift = builder
        .build_int_sub(
            exponent,
            i128_ty
                .const_int(u64::from(precision - 1), false)
                .map_err(diagnostic_from_llvm_error)?,
            "right_shift",
        )
        .map_err(diagnostic_from_llvm_error)?;
    let significand = builder
        .build_right_shift(magnitude, shift, false, "significand")
        .map_err(diagnostic_from_llvm_error)?;
    let half_shift = builder
        .build_int_sub(
            shift,
            i128_ty
                .const_int(1, false)
                .map_err(diagnostic_from_llvm_error)?,
            "half_shift",
        )
        .map_err(diagnostic_from_llvm_error)?;
    let one = i128_ty
        .const_int(1, false)
        .map_err(diagnostic_from_llvm_error)?;
    let halfway = builder
        .build_left_shift(one, half_shift, "halfway")
        .map_err(diagnostic_from_llvm_error)?;
    let remainder_mask = builder
        .build_left_shift(one, shift, "remainder_limit")
        .and_then(|limit| builder.build_int_sub(limit, one, "remainder_mask"))
        .map_err(diagnostic_from_llvm_error)?;
    let remainder = builder
        .build_and(magnitude, remainder_mask, "remainder")
        .map_err(diagnostic_from_llvm_error)?;
    let above_half = builder
        .build_int_compare(IntPredicate::UGT, remainder, halfway, "above_half")
        .map_err(diagnostic_from_llvm_error)?;
    let exactly_half = builder
        .build_int_compare(IntPredicate::EQ, remainder, halfway, "exactly_half")
        .map_err(diagnostic_from_llvm_error)?;
    let odd = builder
        .build_and(significand, one, "odd_bit")
        .and_then(|odd| {
            builder.build_int_compare(IntPredicate::NE, odd, i128_ty.const_zero()?, "odd")
        })
        .map_err(diagnostic_from_llvm_error)?;
    let tied_odd = builder
        .build_and(exactly_half, odd, "tied_odd")
        .map_err(diagnostic_from_llvm_error)?;
    let round_up = builder
        .build_or(above_half, tied_odd, "round_up")
        .map_err(diagnostic_from_llvm_error)?;
    let round_increment = builder
        .build_int_z_extend(round_up, i128_ty, "round_increment")
        .map_err(diagnostic_from_llvm_error)?;
    let rounded = builder
        .build_int_add(significand, round_increment, "rounded")
        .map_err(diagnostic_from_llvm_error)?;
    let carried = builder
        .build_int_compare(
            IntPredicate::EQ,
            rounded,
            i128_ty
                .const_u128(1_u128 << precision)
                .map_err(diagnostic_from_llvm_error)?,
            "carried",
        )
        .map_err(diagnostic_from_llvm_error)?;
    let carried_significand = builder
        .build_right_shift(rounded, one, false, "carried_significand")
        .map_err(diagnostic_from_llvm_error)?;
    let final_significand = builder
        .build_select(
            carried.into(),
            carried_significand.into(),
            rounded.into(),
            "final_significand",
        )
        .and_then(|value| value.into_int_value())
        .map_err(diagnostic_from_llvm_error)?;
    let incremented_exponent = builder
        .build_int_add(exponent, one, "incremented_exponent")
        .map_err(diagnostic_from_llvm_error)?;
    let final_exponent = builder
        .build_select(
            carried.into(),
            incremented_exponent.into(),
            exponent.into(),
            "final_exponent",
        )
        .and_then(|value| value.into_int_value())
        .map_err(diagnostic_from_llvm_error)?;
    emit_i128_to_float_return(
        &builder,
        i128_ty,
        format,
        final_significand,
        final_exponent,
        is_negative,
    )
}

#[derive(Clone, Copy)]
struct I128ToFloatFormat<'ctx> {
    target_ty: nia_llvm::types::FloatType<'ctx>,
    storage_ty: nia_llvm::types::IntType<'ctx>,
    exponent_bias: u64,
    fraction_bits: u32,
    storage_bits: u32,
}

fn emit_i128_to_float_return<'ctx>(
    builder: &nia_llvm::builder::Builder<'ctx>,
    i128_ty: nia_llvm::types::IntType<'ctx>,
    format: I128ToFloatFormat<'ctx>,
    significand: IntValue<'ctx>,
    exponent: IntValue<'ctx>,
    is_negative: IntValue<'ctx>,
) -> Result<(), Diagnostic> {
    let fraction_mask = i128_ty
        .const_u128((1_u128 << format.fraction_bits) - 1)
        .map_err(diagnostic_from_llvm_error)?;
    let fraction = builder
        .build_and(significand, fraction_mask, "fraction")
        .and_then(|fraction| {
            builder.build_int_truncate(fraction, format.storage_ty, "fraction.bits")
        })
        .map_err(diagnostic_from_llvm_error)?;
    let exponent = builder
        .build_int_truncate(exponent, format.storage_ty, "exponent.bits")
        .and_then(|exponent| {
            builder.build_int_add(
                exponent,
                format.storage_ty.const_int(format.exponent_bias, false)?,
                "biased_exponent",
            )
        })
        .and_then(|exponent| {
            builder.build_left_shift(
                exponent,
                format
                    .storage_ty
                    .const_int(u64::from(format.fraction_bits), false)?,
                "exponent.field",
            )
        })
        .map_err(diagnostic_from_llvm_error)?;
    let sign = builder
        .build_int_z_extend(is_negative, format.storage_ty, "sign.bits")
        .and_then(|sign| {
            builder.build_left_shift(
                sign,
                format
                    .storage_ty
                    .const_int(u64::from(format.storage_bits - 1), false)?,
                "sign.field",
            )
        })
        .map_err(diagnostic_from_llvm_error)?;
    let bits = builder
        .build_or(exponent, fraction, "magnitude.bits")
        .and_then(|magnitude| builder.build_or(sign, magnitude, "result.bits"))
        .map_err(diagnostic_from_llvm_error)?;
    let result = builder
        .build_bit_cast(bits, format.target_ty, "result")
        .and_then(|value| value.into_float_value())
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_return(Some(&result))
        .map(|_| ())
        .map_err(diagnostic_from_llvm_error)
}

fn emit_i128_from_float<'ctx>(
    context: &'ctx Context,
    module: &nia_llvm::module::Module<'ctx>,
    source: PrimitiveTy,
    signed: bool,
) -> Result<(), Diagnostic> {
    let source_ty = match source {
        PrimitiveTy::F32 => context.f32_type(),
        PrimitiveTy::F64 => context.f64_type(),
        _ => {
            return Err(diagnostic_from_llvm_error(LlvmError::ice(
                "i128 float conversion builtin requires f32 or f64",
            )));
        }
    };
    let i64_ty = context.i64_type();
    let i128_ty = context.i128_type();
    let fn_ty = i128_ty
        .fn_type(&[source_ty.into()], false)
        .map_err(diagnostic_from_llvm_error)?;
    let function = module
        .add_function(
            match source {
                PrimitiveTy::F32 if signed => "__fixsfti",
                PrimitiveTy::F64 if signed => "__fixdfti",
                PrimitiveTy::F32 => "__fixunssfti",
                PrimitiveTy::F64 => "__fixunsdfti",
                _ => unreachable!("source primitive checked above"),
            },
            fn_ty,
            Some(Linkage::External),
        )
        .map_err(diagnostic_from_llvm_error)?;
    let entry = context
        .append_basic_block(function, "entry")
        .map_err(diagnostic_from_llvm_error)?;
    let builder = context
        .create_builder()
        .map_err(diagnostic_from_llvm_error)?;
    builder.position_at_end(entry);
    let value = function
        .get_nth_param(0)
        .ok_or_else(|| diagnostic_from_llvm_error(LlvmError::ice("missing builtin param")))?
        .map_err(diagnostic_from_llvm_error)?
        .into_float_value()
        .map_err(diagnostic_from_llvm_error)?;
    let (magnitude, is_negative) = if signed {
        let is_negative = builder
            .build_float_compare(
                FloatPredicate::OLT,
                value,
                source_ty.const_zero().map_err(diagnostic_from_llvm_error)?,
                "is_negative",
            )
            .map_err(diagnostic_from_llvm_error)?;
        let negative = builder
            .build_float_neg(value, "negative")
            .map_err(diagnostic_from_llvm_error)?;
        let magnitude = builder
            .build_select(
                is_negative.into(),
                negative.into(),
                value.into(),
                "magnitude",
            )
            .and_then(|value| value.into_float_value())
            .map_err(diagnostic_from_llvm_error)?;
        (magnitude, Some(is_negative))
    } else {
        (value, None)
    };
    let two_to_64 = source_ty
        .const_float(18_446_744_073_709_551_616.0)
        .map_err(diagnostic_from_llvm_error)?;
    let high_float = builder
        .build_float_div(magnitude, two_to_64, "high.float")
        .map_err(diagnostic_from_llvm_error)?;
    let high = builder
        .build_float_to_unsigned_int(high_float, i64_ty, "high")
        .map_err(diagnostic_from_llvm_error)?;
    let high_as_float = builder
        .build_unsigned_int_to_float(high, source_ty, "high.as_float")
        .map_err(diagnostic_from_llvm_error)?;
    let high_value = builder
        .build_float_mul(high_as_float, two_to_64, "high.value")
        .map_err(diagnostic_from_llvm_error)?;
    let low_float = builder
        .build_float_sub(magnitude, high_value, "low.float")
        .map_err(diagnostic_from_llvm_error)?;
    let low = builder
        .build_float_to_unsigned_int(low_float, i64_ty, "low")
        .map_err(diagnostic_from_llvm_error)?;
    let high = builder
        .build_int_z_extend(high, i128_ty, "high.wide")
        .and_then(|high| {
            builder.build_left_shift(high, i128_ty.const_int(64, false)?, "high.shifted")
        })
        .map_err(diagnostic_from_llvm_error)?;
    let low = builder
        .build_int_z_extend(low, i128_ty, "low.wide")
        .map_err(diagnostic_from_llvm_error)?;
    let magnitude = builder
        .build_or(high, low, "result")
        .map_err(diagnostic_from_llvm_error)?;
    let result = if let Some(is_negative) = is_negative {
        let negative = builder
            .build_int_neg(magnitude, "result.negative")
            .map_err(diagnostic_from_llvm_error)?;
        builder
            .build_select(
                is_negative.into(),
                negative.into(),
                magnitude.into(),
                "result.signed",
            )
            .and_then(|value| value.into_int_value())
            .map_err(diagnostic_from_llvm_error)?
    } else {
        magnitude
    };
    builder
        .build_return(Some(&result))
        .map_err(diagnostic_from_llvm_error)?;
    Ok(())
}

fn emit_u128_div_rem<'ctx>(
    context: &'ctx Context,
    module: &nia_llvm::module::Module<'ctx>,
    signed: bool,
) -> Result<(), Diagnostic> {
    let i128_ty = context.i128_type();
    let fn_ty = i128_ty
        .fn_type(&[i128_ty.into(), i128_ty.into()], false)
        .map_err(diagnostic_from_llvm_error)?;
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

    let divmod_ty = i128_ty
        .fn_type(
            &[i128_ty.into(), i128_ty.into(), context.bool_type().into()],
            false,
        )
        .map_err(diagnostic_from_llvm_error)?;
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

    let builder = context
        .create_builder()
        .map_err(diagnostic_from_llvm_error)?;
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
        .build_store(
            quotient,
            i128_ty.const_zero().map_err(diagnostic_from_llvm_error)?,
        )
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_store(
            remainder,
            i128_ty.const_zero().map_err(diagnostic_from_llvm_error)?,
        )
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_store(
            shift,
            i128_ty
                .const_int(128, false)
                .map_err(diagnostic_from_llvm_error)?,
        )
        .map_err(diagnostic_from_llvm_error)?;
    let div_by_zero = builder
        .build_int_compare(
            IntPredicate::EQ,
            b,
            i128_ty.const_zero().map_err(diagnostic_from_llvm_error)?,
            "divzero",
        )
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
            i128_ty.const_zero().map_err(diagnostic_from_llvm_error)?,
            "keepgoing",
        )
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_conditional_branch(keep_going, body_block, done_block)
        .map_err(diagnostic_from_llvm_error)?;

    builder.position_at_end(body_block);
    let next_shift = builder
        .build_int_sub(
            current_shift,
            i128_ty
                .const_int(1, false)
                .map_err(diagnostic_from_llvm_error)?,
            "nextshift",
        )
        .map_err(diagnostic_from_llvm_error)?;
    builder
        .build_store(shift, next_shift)
        .map_err(diagnostic_from_llvm_error)?;
    let rem_value = load_i128(&builder, i128_ty, remainder, "rem.load")?;
    let rem_shifted = builder
        .build_left_shift(
            rem_value,
            i128_ty
                .const_int(1, false)
                .map_err(diagnostic_from_llvm_error)?,
            "rem.shift",
        )
        .map_err(diagnostic_from_llvm_error)?;
    let one = i128_ty
        .const_int(1, false)
        .map_err(diagnostic_from_llvm_error)?;
    let bit = builder
        .build_right_shift(a, next_shift, false, "bit.shift")
        .and_then(|value| builder.build_and(value, one, "bit"))
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
        .build_left_shift(
            i128_ty
                .const_int(1, false)
                .map_err(diagnostic_from_llvm_error)?,
            next_shift,
            "quo.bit",
        )
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
    let builder = context
        .create_builder()
        .map_err(diagnostic_from_llvm_error)?;
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
        i1_ty
            .const_int(want_rem as u64, false)
            .map_err(diagnostic_from_llvm_error)?
            .into(),
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
    let builder = context
        .create_builder()
        .map_err(diagnostic_from_llvm_error)?;
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
    let zero = i128_ty.const_zero().map_err(diagnostic_from_llvm_error)?;
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
        i1_ty
            .const_int(want_rem as u64, false)
            .map_err(diagnostic_from_llvm_error)?
            .into(),
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
