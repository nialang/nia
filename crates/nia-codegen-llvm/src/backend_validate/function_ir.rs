// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashSet;

use nia_ast::{AssignOp, BinaryOp, UnaryOp};
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    AtomicOrder, FunctionArrayElements, FunctionBody, FunctionCallee, FunctionDeferBody,
    FunctionExpr, FunctionExprKind, FunctionOp, FunctionPlace, FunctionPlaceBase,
    FunctionPlaceElem, FunctionTerminator, FunctionTryKind, validate_function_body,
};
use nia_mangle::mangle_symbol_id;
use nia_span::Span;
use nia_ty::{ConstGenericArg, ConstGenericValue, PrimitiveTy, TyKind};

use crate::literals::{
    assign_to_binary_op, decode_byte_char_literal, parse_float_literal, parse_int_literal,
};

mod atomics;
mod calls;
mod places;

use super::{BackendValidator, FunctionInstanceRef};

struct DynamicTraitCallContract<'a> {
    object_ty: nia_ids::InternedTyId,
    trait_id: nia_ty::TraitId,
    method_id: nia_ids::GlobalDefId,
    trait_args: &'a [nia_ids::InternedTyId],
    trait_const_args: &'a [ConstGenericArg],
    slot: usize,
    params: &'a [nia_ids::InternedTyId],
    return_type: nia_ids::InternedTyId,
    receiver_kind: nia_ids::ReceiverKind,
    receiver: &'a FunctionExpr,
    args: &'a [FunctionExpr],
    result_ty: nia_ids::InternedTyId,
    tracks_caller: bool,
    span: Span,
}

#[derive(Clone, Copy)]
struct VtableTraitInstance<'a> {
    trait_id: nia_ty::TraitId,
    args: &'a [nia_ids::InternedTyId],
    const_args: &'a [ConstGenericArg],
}

pub(super) struct CallTargetSignature {
    params: Vec<nia_backend_ir::BackendParam>,
    return_type: nia_ids::InternedTyId,
    is_variadic: bool,
}

struct TypedCallContract<'a> {
    kind: &'static str,
    args: &'a [FunctionExpr],
    params: &'a [nia_ids::InternedTyId],
    return_type: nia_ids::InternedTyId,
    is_variadic: bool,
    result_ty: nia_ids::InternedTyId,
    span: Span,
}

#[derive(Clone, Copy)]
enum AtomicOrderContext {
    Load,
    Store,
    Rmw,
    CmpxchgSuccess,
    CmpxchgFailure,
    Fence,
}

#[derive(Clone, Copy)]
enum TaggedUnionConstructor {
    OptionalSome,
    ErrorOk,
    ErrorErr,
}

#[derive(Clone, Copy)]
enum TaggedUnionProjection {
    Tag,
    Payload,
}

struct SliceSourceInfo {
    elem: nia_ids::InternedTyId,
    readonly: bool,
}

impl AtomicOrderContext {
    fn allows(self, order: AtomicOrder) -> bool {
        match self {
            Self::Load => matches!(
                order,
                AtomicOrder::Unordered
                    | AtomicOrder::Monotonic
                    | AtomicOrder::Acquire
                    | AtomicOrder::SeqCst
            ),
            Self::Store => matches!(
                order,
                AtomicOrder::Unordered
                    | AtomicOrder::Monotonic
                    | AtomicOrder::Release
                    | AtomicOrder::SeqCst
            ),
            Self::Rmw | Self::CmpxchgSuccess => matches!(
                order,
                AtomicOrder::Monotonic
                    | AtomicOrder::Acquire
                    | AtomicOrder::Release
                    | AtomicOrder::AcqRel
                    | AtomicOrder::SeqCst
            ),
            Self::CmpxchgFailure => matches!(
                order,
                AtomicOrder::Monotonic | AtomicOrder::Acquire | AtomicOrder::SeqCst
            ),
            Self::Fence => matches!(
                order,
                AtomicOrder::Acquire
                    | AtomicOrder::Release
                    | AtomicOrder::AcqRel
                    | AtomicOrder::SeqCst
            ),
        }
    }
}

impl BackendValidator<'_> {
    pub(super) fn validate_function_body(
        &mut self,
        body: &FunctionBody,
        expected_return_ty: nia_ids::InternedTyId,
    ) {
        if let Err(error) = validate_function_body(body) {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                error.span,
                format!("backend IR contains invalid function IR: {}", error.message),
            ));
            return;
        }
        self.current_subject = Some("body result");
        self.validate_runtime_type(body.ty, body.span);
        self.current_subject = None;
        self.local_tys.push(
            body.locals
                .iter()
                .map(|local| (local.id, local.ty))
                .collect(),
        );
        self.local_kinds.push(
            body.locals
                .iter()
                .map(|local| (local.id, local.kind))
                .collect(),
        );
        // `FunctionBody::ty` describes the lowered block expression and may be
        // `Never` for a terminating builtin even when the declared function
        // return type is a concrete value. Propagation contracts use the
        // enclosing function signature, not that intermediate expression type.
        self.body_tys.push(expected_return_ty);
        for local in &body.locals {
            self.current_subject = Some("local");
            self.validate_runtime_type(local.ty, local.span);
            self.current_subject = None;
        }
        for block in &body.blocks {
            for op in &block.ops {
                self.validate_op(op);
            }
            self.validate_terminator(&block.terminator);
            self.validate_body_return_contract(
                &block.terminator,
                body.ty,
                expected_return_ty,
                false,
            );
        }
        self.body_tys.pop();
        self.local_kinds.pop();
        self.local_tys.pop();
    }

    fn validate_defer_body(&mut self, body: &FunctionDeferBody) {
        let Some(body_ty) = self.body_tys.last().copied() else {
            return;
        };
        for block in &body.blocks {
            for op in &block.ops {
                self.validate_op(op);
            }
            self.validate_terminator(&block.terminator);
            self.validate_body_return_contract(&block.terminator, body_ty, body_ty, true);
        }
    }

    fn validate_op(&mut self, op: &FunctionOp) {
        match op {
            FunctionOp::Binding(binding) => {
                self.current_subject = Some("binding");
                self.validate_runtime_type(binding.ty, Span::default());
                self.current_subject = None;
                self.validate_local_type(
                    binding.local_id,
                    binding.ty,
                    Span::default(),
                    "binding type does not match its body local",
                );
                if let Some(value) = &binding.value {
                    self.validate_expr(value);
                    if !self.same_type(binding.ty, value.ty) {
                        self.invalid_local_type(
                            value.span,
                            "binding initializer type does not match its binding",
                        );
                    }
                }
            }
            FunctionOp::StoreLocal {
                local_id,
                value,
                span,
            } => {
                self.validate_expr(value);
                self.validate_local_type(
                    *local_id,
                    value.ty,
                    *span,
                    "stored value type does not match its body local",
                );
            }
            FunctionOp::Expr(value) => self.validate_expr(value),
            FunctionOp::MemoryIntrinsic(memory) => self.validate_memory_intrinsic(memory),
            FunctionOp::Defer(body) => self.validate_defer_body(body),
        }
    }

    fn validate_memory_intrinsic(&mut self, memory: &nia_function_ir::FunctionMemoryIntrinsic) {
        use nia_function_ir::{FunctionMemoryIntrinsicOp as Op, FunctionMemoryIntrinsicSource};

        // Codegen derives a raw byte count from `elem_ty` and extracts pointer
        // and length fields from both operands. Keep those independent pieces
        // of producer metadata coherent before any typed LLVM operation runs.
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

    fn validate_terminator(&mut self, terminator: &FunctionTerminator) {
        match terminator {
            FunctionTerminator::If { cond, span, .. } => {
                self.validate_expr(cond);
                self.validate_bool_condition(cond.ty, *span);
            }
            FunctionTerminator::Switch { target, arms, .. } => {
                self.validate_expr(target);
                let mut case_values = HashSet::new();
                for arm in arms {
                    self.validate_expr(&arm.pattern);
                    if !self.same_type(target.ty, arm.pattern.ty) {
                        self.invalid_terminator(
                            arm.pattern.span,
                            "switch arm pattern type does not match its target",
                        );
                    }
                    match self.switch_case_value(&arm.pattern) {
                        Some(value) if !case_values.insert(value) => self.invalid_terminator(
                            arm.pattern.span,
                            "switch contains duplicate case values",
                        ),
                        Some(_) => {}
                        None => self.invalid_terminator(
                            arm.pattern.span,
                            "switch arm pattern is not a compile-time integer constant",
                        ),
                    }
                }
                if !self.is_integer_type(target.ty) {
                    self.invalid_terminator(target.span, "switch target must have an integer type");
                }
            }
            FunctionTerminator::Try {
                value,
                kind,
                error_conversion,
                success_local,
                span,
                ..
            } => {
                self.validate_expr(value);
                if let Some(conversion) = error_conversion {
                    self.validate_expr(conversion);
                }
                self.validate_try_contract(
                    value,
                    *kind,
                    error_conversion.as_deref(),
                    *success_local,
                    *span,
                );
            }
            FunctionTerminator::Loop { header, span, .. } => match header {
                nia_function_ir::FunctionForHeader::Infinite => {}
                nia_function_ir::FunctionForHeader::Condition(expr) => {
                    self.validate_expr(expr);
                    self.validate_bool_condition(expr.ty, *span);
                }
            },
            FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
                if let Some(value) = value {
                    self.validate_expr(value);
                }
            }
            FunctionTerminator::Error { .. }
            | FunctionTerminator::Branch { .. }
            | FunctionTerminator::Next { .. } => {}
        }
    }

    fn validate_body_return_contract(
        &mut self,
        terminator: &FunctionTerminator,
        body_ty: nia_ids::InternedTyId,
        expected_return_ty: nia_ids::InternedTyId,
        in_defer: bool,
    ) {
        let (value, span, is_tail) = match terminator {
            FunctionTerminator::Return { value, span } => (value.as_ref(), *span, false),
            FunctionTerminator::Tail { value, span } => (value.as_ref(), *span, true),
            _ => return,
        };
        // A defer Tail exits only the mini-CFG; it does not return the enclosing
        // function. Its optional value is therefore intentionally unchecked here.
        if in_defer && is_tail {
            return;
        }
        match value {
            Some(value)
                if !self.same_type(value.ty, expected_return_ty)
                    && !matches!(
                        self.ty_kind(value.ty),
                        Some(TyKind::Primitive(PrimitiveTy::Never))
                    ) =>
            {
                self.invalid_terminator(
                    value.span,
                    "return value type does not match the function body return type",
                )
            }
            Some(_) => {}
            None if !self.is_unit_or_never(expected_return_ty)
                && !matches!(
                    self.ty_kind(body_ty),
                    Some(TyKind::Primitive(PrimitiveTy::Never))
                ) =>
            {
                self.invalid_terminator(
                    span,
                    "empty return terminator requires a unit or never return type",
                )
            }
            None => {}
        }
    }

    fn validate_bool_condition(&mut self, ty: nia_ids::InternedTyId, span: Span) {
        if !matches!(self.ty_kind(ty), Some(TyKind::Primitive(PrimitiveTy::Bool))) {
            self.invalid_terminator(span, "control-flow condition must have type bool");
        }
    }

    fn is_integer_type(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(
            self.ty_kind(ty),
            Some(TyKind::Primitive(
                PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::I128
                    | PrimitiveTy::Isize
                    | PrimitiveTy::U8
                    | PrimitiveTy::U16
                    | PrimitiveTy::U32
                    | PrimitiveTy::U64
                    | PrimitiveTy::U128
                    | PrimitiveTy::Usize
                    | PrimitiveTy::Bool
                    | PrimitiveTy::Char
            ))
        )
    }

    /// Returns the case's LLVM integer bit pattern at its target width.
    ///
    /// Function lowering normally reduces integer and boolean patterns to
    /// `Integer` and enum patterns to `EnumVariantTag`. The other literal forms
    /// are retained because they are also directly representable LLVM integer
    /// constants. Keeping this allowlist here prevents a runtime expression
    /// from reaching `LLVMBuildSwitch`, whose case operands must be constants.
    fn switch_case_value(&self, pattern: &FunctionExpr) -> Option<u128> {
        use nia_function_ir::FunctionBuiltinValue;

        let value = match &pattern.kind {
            FunctionExprKind::Integer(text) => parse_int_literal(text)?.bits(),
            FunctionExprKind::Char(value) => u128::from(*value),
            FunctionExprKind::ByteChar(text) => u128::from(decode_byte_char_literal(text)?),
            FunctionExprKind::Bool(value) => u128::from(*value),
            FunctionExprKind::BuiltinValue(FunctionBuiltinValue::Int(value)) => value.bits(),
            FunctionExprKind::EnumVariantTag(variant) => {
                let info = self.index.enum_variant_info(*variant)?;
                info.variant.value.unwrap_or(info.index as i128) as u128
            }
            _ => return None,
        };
        let bits = self.switch_integer_bits(pattern.ty)?;
        let mask = if bits == u128::BITS {
            u128::MAX
        } else {
            (1_u128 << bits) - 1
        };
        Some(value & mask)
    }

    fn switch_integer_bits(&self, ty: nia_ids::InternedTyId) -> Option<u32> {
        let Some(TyKind::Primitive(primitive)) = self.ty_kind(ty) else {
            return None;
        };
        match primitive {
            PrimitiveTy::Bool => Some(1),
            PrimitiveTy::Char => Some(32),
            PrimitiveTy::Isize | PrimitiveTy::Usize => self
                .target
                .pointer_size
                .checked_mul(8)
                .and_then(|bits| u32::try_from(bits).ok()),
            _ => primitive.integer_bits(0),
        }
    }

    fn is_unit_or_never(&self, ty: nia_ids::InternedTyId) -> bool {
        match self.ty_kind(ty) {
            Some(TyKind::Primitive(PrimitiveTy::Never)) => true,
            Some(TyKind::Tuple(fields)) => fields.is_empty(),
            _ => false,
        }
    }

    fn invalid_terminator(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR contains an invalid terminator contract: {message}"),
        ));
    }

    fn validate_try_contract(
        &mut self,
        value: &FunctionExpr,
        kind: FunctionTryKind,
        error_conversion: Option<&FunctionExpr>,
        success_local: nia_ids::LocalId,
        span: Span,
    ) {
        let value_kind = self.ty_kind(value.ty).cloned();
        let (source_error_ty, success_ty) = match (kind, value_kind) {
            (FunctionTryKind::Optional, Some(TyKind::Optional { elem })) => (None, elem),
            (FunctionTryKind::ErrorUnion, Some(TyKind::ErrorUnion { error, value })) => {
                (Some(error), value)
            }
            _ => {
                self.invalid_try(span, "propagation kind does not match its input union type");
                return;
            }
        };

        if let Some(local_ty) = self
            .local_tys
            .last()
            .and_then(|locals| locals.get(&success_local))
            .copied()
            && !self.same_type(local_ty, success_ty)
        {
            self.invalid_try(
                span,
                "propagation success local type does not match the input success payload",
            );
        }

        let Some(body_ty) = self.body_tys.last().copied() else {
            self.invalid_try(span, "propagation is not owned by a function body");
            return;
        };
        match (kind, self.ty_kind(body_ty).cloned()) {
            (FunctionTryKind::Optional, Some(TyKind::Optional { .. })) => {
                if error_conversion.is_some() {
                    self.invalid_try(span, "optional propagation carries an error conversion");
                }
            }
            (
                FunctionTryKind::ErrorUnion,
                Some(TyKind::ErrorUnion {
                    error: target_error_ty,
                    ..
                }),
            ) => {
                let propagated_error_ty = error_conversion
                    .map(|conversion| conversion.ty)
                    .or(source_error_ty);
                if !propagated_error_ty
                    .is_some_and(|error_ty| self.same_type(error_ty, target_error_ty))
                {
                    let message = if error_conversion.is_some() {
                        "propagation conversion type does not match the return error payload"
                    } else {
                        "direct propagation error type does not match the return error payload"
                    };
                    self.invalid_try(span, message);
                }
            }
            _ => self.invalid_try(
                span,
                "propagation kind does not match the function return union",
            ),
        }
    }

    fn invalid_try(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR contains invalid propagation: {message}"),
        ));
    }

    fn validate_expr(&mut self, expr: &FunctionExpr) {
        if matches!(expr.kind, FunctionExprKind::Error) {
            let context = match self.current_item.as_deref() {
                Some(item) => format!(" in {item}"),
                None => String::new(),
            };
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                expr.span,
                format!("backend IR contains erroneous expression{context}"),
            ));
            return;
        }
        self.current_subject = Some("expr");
        self.validate_runtime_type(expr.ty, expr.span);
        self.current_subject = None;
        match &expr.kind {
            FunctionExprKind::Global(def_id) => {
                if let Some(global) = self.index.global(*def_id) {
                    if !self.projection_result_compatible(expr.ty, global.ty) {
                        self.invalid_global_value_type(expr.span, "global");
                    }
                } else {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        expr.span,
                        format!("backend IR expression references missing global {def_id:?}"),
                    ));
                }
            }
            FunctionExprKind::GlobalInstance {
                def_id,
                arg_module_id,
                args,
                const_args,
            } => {
                if let Some(global) =
                    self.index
                        .global_instance(*def_id, *arg_module_id, args, const_args)
                {
                    if !self.projection_result_compatible(expr.ty, global.ty) {
                        self.invalid_global_value_type(expr.span, "global instance");
                    }
                } else {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        expr.span,
                        format!(
                            "backend IR expression references missing global instance {def_id:?}"
                        ),
                    ));
                }
            }
            FunctionExprKind::Function(def_id) => {
                self.validate_function_ref(
                    *def_id,
                    expr.span,
                    "backend IR expression references missing function",
                );
                if let Some(signature) = self.function_call_signature(*def_id) {
                    self.validate_function_value_signature(
                        "function", expr.ty, &signature, expr.span,
                    );
                }
            }
            FunctionExprKind::EnumConstructor(variant) => {
                self.validate_enum_constructor(expr.ty, *variant, expr.span);
            }
            FunctionExprKind::FunctionInstance {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
            } => {
                let instance = FunctionInstanceRef {
                    def_id: *def_id,
                    arg_module_id: *arg_module_id,
                    self_arg: *self_arg,
                    args,
                    const_args,
                };
                self.validate_function_instance_ref(
                    instance,
                    expr.span,
                    "backend IR expression references missing function instance",
                );
                if let Some(signature) = self.function_instance_call_signature(instance) {
                    self.validate_function_value_signature(
                        "function-instance",
                        expr.ty,
                        &signature,
                        expr.span,
                    );
                }
            }
            FunctionExprKind::Range(range) => {
                self.validate_range_expr(expr.ty, range, expr.span);
            }
            FunctionExprKind::RangeBound { range, bound } => {
                self.validate_range_bound(expr.ty, range, *bound, expr.span);
            }
            FunctionExprKind::InlineAsm(asm) => self.validate_inline_asm(expr.ty, asm, expr.span),
            FunctionExprKind::Atomic(atomic) => self.validate_atomic(atomic, expr.ty, expr.span),
            FunctionExprKind::StaticArrayPointer {
                allocation,
                array,
                is_readonly,
            } => {
                // Identity only: the origin module need not be published yet.
                if !self.index.is_registered_module(allocation.module_id()) {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        expr.span,
                        "backend IR static array pointer references a missing origin module",
                    ));
                }
                self.validate_static_array_pointer(expr.ty, array, *is_readonly, expr.span);
            }
            FunctionExprKind::ArrayLiteral { elems } => {
                let array_contract = match self.index.ty_kind(expr.ty).cloned() {
                    Some(TyKind::Array { len, elem }) => Some((len, elem)),
                    _ => {
                        self.invalid_literal_contract(
                            expr.span,
                            "array",
                            "expression type is not an array",
                        );
                        None
                    }
                };
                match elems {
                    FunctionArrayElements::List(elems) => {
                        if let Some((len, elem_ty)) = &array_contract {
                            if self
                                .array_len_value(len)
                                .is_some_and(|expected| u64::try_from(elems.len()) != Ok(expected))
                            {
                                self.invalid_literal_contract(
                                    expr.span,
                                    "array",
                                    "element count does not match its type length",
                                );
                            }
                            if elems.iter().any(|elem| !self.same_type(elem.ty, *elem_ty)) {
                                self.invalid_literal_contract(
                                    expr.span,
                                    "array",
                                    "element type does not match its array type",
                                );
                            }
                        }
                        for elem in elems {
                            self.validate_expr(elem);
                        }
                    }
                    FunctionArrayElements::Repeat { value, count } => {
                        self.validate_array_len(count, expr.span);
                        if let Some((len, elem_ty)) = &array_contract {
                            if self.array_len_value(len) != self.array_len_value(count) {
                                self.invalid_literal_contract(
                                    expr.span,
                                    "array repeat",
                                    "count does not match its type length",
                                );
                            }
                            self.validate_projection_result_type(
                                value.ty,
                                *elem_ty,
                                expr.span,
                                "array repeat element",
                            );
                        }
                        self.validate_expr(value);
                    }
                }
            }
            FunctionExprKind::Tuple(elems) => {
                // Closure capture state deliberately reuses tuple construction
                // in function IR; its capture list is therefore the tuple-like
                // element contract at this backend boundary.
                match self.index.ty_kind(expr.ty).cloned() {
                    Some(
                        TyKind::Tuple(expected)
                        | TyKind::ClosureState {
                            captures: expected, ..
                        },
                    ) => {
                        if elems.len() != expected.len() {
                            self.invalid_literal_contract(
                                expr.span,
                                "tuple",
                                "element count does not match its type arity",
                            );
                        }
                        if elems
                            .iter()
                            .zip(expected)
                            .any(|(elem, expected)| !self.same_type(elem.ty, expected))
                        {
                            self.invalid_literal_contract(
                                expr.span,
                                "tuple",
                                "element type does not match its tuple type",
                            );
                        }
                    }
                    _ => self.invalid_literal_contract(
                        expr.span,
                        "tuple",
                        "expression type is not a tuple",
                    ),
                }
                for elem in elems {
                    self.validate_expr(elem);
                }
            }
            FunctionExprKind::TupleField { value, index } => {
                self.validate_expr(value);
                let expected_ty = match self.index.ty_kind(value.ty) {
                    Some(
                        TyKind::Tuple(elems)
                        | TyKind::ClosureState {
                            captures: elems, ..
                        },
                    ) if *index < elems.len() => Some(elems[*index]),
                    Some(TyKind::Tuple(_) | TyKind::ClosureState { .. }) => {
                        self.diagnostics.push(Diagnostic::internal_error_at(
                            nia_diagnostic::codes::INVALID_BACKEND_IR,
                            expr.span,
                            "backend IR tuple projection is out of bounds",
                        ));
                        None
                    }
                    _ => {
                        self.diagnostics.push(Diagnostic::internal_error_at(
                            nia_diagnostic::codes::INVALID_BACKEND_IR,
                            expr.span,
                            "backend IR tuple projection target is not a tuple",
                        ));
                        None
                    }
                };
                if let Some(expected_ty) = expected_ty {
                    self.validate_projection_result_type(expr.ty, expected_ty, expr.span, "tuple");
                }
            }
            FunctionExprKind::StructLiteral { def_id, fields } => {
                self.validate_aggregate_literal_identity("struct", expr.ty, *def_id, expr.span);
                self.validate_struct_literal_field_coverage(expr.ty, fields, expr.span);
                for field in fields {
                    if let Some(expected_ty) =
                        self.validate_field_init(expr.ty, field.field, field.span)
                        && self.has_direct_aggregate_field_contract(expr.ty)
                    {
                        self.validate_projection_result_type(
                            field.value.ty,
                            expected_ty,
                            field.span,
                            "aggregate field initializer",
                        );
                    }
                    self.validate_expr(&field.value);
                }
            }
            FunctionExprKind::UnionLiteral { def_id, field } => {
                self.validate_aggregate_literal_identity("union", expr.ty, *def_id, expr.span);
                if let Some(expected_ty) =
                    self.validate_field_init(expr.ty, field.field, field.span)
                    && self.has_direct_aggregate_field_contract(expr.ty)
                {
                    self.validate_projection_result_type(
                        field.value.ty,
                        expected_ty,
                        field.span,
                        "union field initializer",
                    );
                }
                self.validate_expr(&field.value);
            }
            FunctionExprKind::UnionStorageLiteral { bytes, relocations } => {
                let is_union = match self.index.ty_kind(expr.ty) {
                    Some(TyKind::Nominal { def_id, .. }) => {
                        self.index.has_union(*def_id) || self.index.has_union_instances(*def_id)
                    }
                    _ => false,
                };
                if !is_union {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        expr.span,
                        "backend IR union storage literal has a non-union type",
                    ));
                }
                let expected_size = self
                    .index
                    .type_layout(expr.ty)
                    .and_then(|layout| usize::try_from(layout.size).ok());
                if expected_size != Some(bytes.len()) {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        expr.span,
                        "backend IR union storage literal has the wrong byte length",
                    ));
                }
                for relocation in relocations {
                    let owner_id = relocation.allocation.module_id();
                    if !self.index.is_registered_module(owner_id) {
                        self.diagnostics.push(Diagnostic::internal_error_at(
                            nia_diagnostic::codes::INVALID_BACKEND_IR,
                            expr.span,
                            "backend IR union storage relocation references a missing module",
                        ));
                        continue;
                    }
                    // The pointer width comes from the owner's payload; defer the
                    // comparison while a registered owner is still unwritten.
                    if let Some(owner) = self.index.written_module(owner_id)
                        && usize::try_from(owner.layouts.target.pointer_size).ok()
                            != Some(relocation.width)
                    {
                        self.diagnostics.push(Diagnostic::internal_error_at(
                            nia_diagnostic::codes::INVALID_BACKEND_IR,
                            expr.span,
                            "backend IR union storage relocation has the wrong pointer width",
                        ));
                    }
                    self.current_subject = Some("promoted allocation");
                    self.validate_runtime_type(relocation.pointee.ty, relocation.pointee.span);
                    self.current_subject = None;
                    self.validate_expr(&relocation.pointee);
                }
            }
            FunctionExprKind::OptionalSome { expr: inner } => {
                self.validate_tagged_union_constructor(
                    expr.ty,
                    inner,
                    TaggedUnionConstructor::OptionalSome,
                    expr.span,
                );
            }
            FunctionExprKind::ErrorOk { expr: inner } => {
                self.validate_tagged_union_constructor(
                    expr.ty,
                    inner,
                    TaggedUnionConstructor::ErrorOk,
                    expr.span,
                );
            }
            FunctionExprKind::ErrorErr { expr: inner } => {
                self.validate_tagged_union_constructor(
                    expr.ty,
                    inner,
                    TaggedUnionConstructor::ErrorErr,
                    expr.span,
                );
            }
            FunctionExprKind::TaggedUnionTag { expr: inner } => {
                self.validate_tagged_union_projection(
                    expr.ty,
                    inner,
                    TaggedUnionProjection::Tag,
                    expr.span,
                );
            }
            FunctionExprKind::TaggedUnionPayload { expr: inner } => {
                self.validate_tagged_union_projection(
                    expr.ty,
                    inner,
                    TaggedUnionProjection::Payload,
                    expr.span,
                );
            }
            FunctionExprKind::Try { expr: inner } => {
                self.validate_expr(inner);
                self.invalid_try(
                    expr.span,
                    "propagation expression was not lowered to a CFG terminator",
                );
            }
            FunctionExprKind::Discard(inner) => {
                self.validate_expr(inner);
                if !matches!(self.ty_kind(expr.ty), Some(TyKind::Tuple(elems)) if elems.is_empty())
                {
                    self.invalid_projection(expr.span, "discard result type is not unit");
                }
            }
            FunctionExprKind::TraitObjectUpcast {
                expr: inner,
                source_ty,
                target_ty,
            } => {
                self.validate_trait_object_upcast(expr.ty, inner, *source_ty, *target_ty, expr.span)
            }
            FunctionExprKind::TraitObjectCoercion {
                expr: inner,
                target_ty,
                self_ty,
            } => {
                self.validate_trait_object_coercion(expr.ty, inner, *self_ty, *target_ty, expr.span)
            }
            FunctionExprKind::Cast { expr: inner, ty } => {
                self.validate_cast(expr.ty, *ty, inner, expr.span);
            }
            FunctionExprKind::LoadUnaligned { ty, ptr } => {
                self.validate_load_unaligned(expr.ty, *ty, ptr, expr.span)
            }
            FunctionExprKind::Splat { value } => self.validate_splat(expr.ty, value, expr.span),
            FunctionExprKind::Bitmask { vector } => {
                self.validate_bitmask(expr.ty, vector, expr.span)
            }
            FunctionExprKind::BitIntrinsic { value, .. } => {
                self.validate_bit_intrinsic(expr.ty, value, expr.span)
            }
            FunctionExprKind::CharFromU32 { value } => {
                self.validate_char_from_u32(expr.ty, value, expr.span)
            }
            FunctionExprKind::CallableCoercion { state, closure_id } => {
                self.validate_callable_coercion(expr.ty, state, *closure_id, expr.span);
            }
            FunctionExprKind::FunctionCallable { function } => {
                self.validate_expr(function);
                if !matches!(
                    self.ty_kind(function.ty),
                    Some(TyKind::FunctionPointer {
                        is_variadic: false,
                        ..
                    })
                ) || !matches!(self.ty_kind(expr.ty), Some(TyKind::Callable { .. }))
                {
                    self.invalid_projection(
                        expr.span,
                        "function callable has incompatible source or target type",
                    );
                }
            }
            FunctionExprKind::Unary { op, expr: inner } => {
                self.validate_unary(expr.ty, *op, inner, expr.span);
            }
            FunctionExprKind::ClosureFunctionPointer { closure_id } => {
                self.validate_closure_function_pointer(expr.ty, *closure_id, expr.span);
            }
            FunctionExprKind::AddrOf(place) => {
                self.validate_place(place);
                self.validate_addr_of_result(expr.ty, place, expr.span);
            }
            FunctionExprKind::Binary { lhs, op, rhs } => {
                self.validate_binary(expr.ty, lhs, *op, rhs, expr.span);
            }
            FunctionExprKind::ExtractElement { vector, index } => {
                self.validate_vector_element(expr.ty, vector, index, None, expr.span);
            }
            FunctionExprKind::InsertElement {
                vector,
                index,
                value,
            } => {
                self.validate_vector_element(expr.ty, vector, index, Some(value), expr.span);
            }
            FunctionExprKind::Assign { place, op, rhs } => {
                self.validate_assignment(expr.ty, place, *op, rhs, expr.span);
            }
            FunctionExprKind::Call { callee, args } => {
                self.validate_callee(callee, args, expr.ty, expr.span);
                for arg in args {
                    self.validate_expr(arg);
                }
            }
            FunctionExprKind::Field { lhs, field } => {
                self.validate_expr(lhs);
                if let Some(expected_ty) = self.validate_aggregate_field(
                    lhs.ty,
                    *field,
                    expr.span,
                    "backend IR field expression references missing field",
                ) && self.has_direct_aggregate_field_contract(lhs.ty)
                {
                    self.validate_projection_result_type(expr.ty, expected_ty, expr.span, "field");
                }
            }
            FunctionExprKind::Index { lhs, index } => {
                self.validate_expr(lhs);
                self.validate_expr(index);
                if !self.is_integer_type(index.ty) {
                    self.invalid_projection(expr.span, "index expression is not integer-like");
                }
                let Some(expected_ty) = self.array_elem_ty(lhs.ty) else {
                    self.invalid_projection(expr.span, "index target is not indexable storage");
                    return;
                };
                self.validate_projection_result_type(expr.ty, expected_ty, expr.span, "index");
            }
            FunctionExprKind::Slice {
                lhs,
                range,
                is_readonly,
            } => {
                self.validate_slice_contract(expr.ty, lhs, *is_readonly, expr.span);
                if let Some(start) = &range.start {
                    self.validate_expr(start);
                    self.validate_slice_bound(start);
                }
                if let Some(end) = &range.end {
                    self.validate_expr(end);
                    self.validate_slice_bound(end);
                }
            }
            FunctionExprKind::Integer(text) => {
                self.validate_integer_literal(expr.ty, text, expr.span)
            }
            FunctionExprKind::Float(text) => self.validate_float_literal(expr.ty, text, expr.span),
            FunctionExprKind::String(scalars) => {
                self.validate_string_scalars(scalars, expr.span);
                self.validate_string_literal(expr.ty, scalars.len(), expr.span, false)
            }
            FunctionExprKind::ByteString(bytes) => {
                self.validate_string_literal(expr.ty, bytes.len(), expr.span, true)
            }
            FunctionExprKind::Char(value) => {
                if !matches!(
                    self.index.ty_kind(expr.ty),
                    Some(TyKind::Primitive(PrimitiveTy::Char))
                ) {
                    self.invalid_literal_contract(expr.span, "char", "target type is not char");
                }
                if char::from_u32(*value).is_none() {
                    self.invalid_literal_contract(
                        expr.span,
                        "char",
                        "value is not a Unicode scalar",
                    );
                }
            }
            FunctionExprKind::ByteChar(text) => {
                if !matches!(
                    self.index.ty_kind(expr.ty),
                    Some(TyKind::Primitive(PrimitiveTy::U8))
                ) {
                    self.invalid_literal_contract(expr.span, "byte char", "target type is not u8");
                }
                if decode_byte_char_literal(text).is_none() {
                    self.invalid_literal_contract(expr.span, "byte char", "spelling is invalid");
                }
            }
            FunctionExprKind::Bool(_) => {
                if !self.is_bool_type(expr.ty) {
                    self.invalid_literal_contract(expr.span, "bool", "target type is not bool");
                }
            }
            FunctionExprKind::Null => {
                if !matches!(
                    self.index.ty_kind(expr.ty),
                    Some(TyKind::Optional { .. } | TyKind::ErrorUnion { .. })
                ) {
                    self.invalid_literal_contract(
                        expr.span,
                        "null",
                        "target type is not Optional or ErrorUnion",
                    );
                }
            }
            FunctionExprKind::BuiltinValue(value) => {
                self.validate_builtin_value(expr.ty, value, expr.span);
            }
            FunctionExprKind::Local(local_id) => {
                let Some(local_ty) = self
                    .local_tys
                    .last()
                    .and_then(|locals| locals.get(local_id))
                    .copied()
                else {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        expr.span,
                        format!("backend IR expression references missing local {local_id:?}"),
                    ));
                    return;
                };
                if !self.projection_result_compatible(expr.ty, local_ty) {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        expr.span,
                        "backend IR local value has an invalid type contract: expression type does not match its storage",
                    ));
                }
            }
            FunctionExprKind::Error | FunctionExprKind::Trap => {}
            FunctionExprKind::ConstGeneric(arg) => {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    expr.span,
                    format!(
                        "backend IR const generic `{}` reached LLVM codegen",
                        self.const_generic_value_name(&arg.value)
                    ),
                ));
            }
            FunctionExprKind::EnumVariant { variant, fields } => {
                self.validate_enum_variant_expr(expr.ty, *variant, fields, expr.span);
            }
            FunctionExprKind::EnumVariantTag(variant) => {
                self.validate_enum_variant_tag(expr.ty, *variant, expr.span);
            }
            FunctionExprKind::EnumTag { value } => {
                self.validate_enum_tag(expr.ty, value, expr.span);
            }
            FunctionExprKind::EnumPayloadField {
                value,
                variant,
                field,
            } => {
                self.validate_enum_payload_field(expr.ty, value, *variant, *field, expr.span);
            }
            FunctionExprKind::CallerLocation(_) => {}
        }
    }

    fn validate_load_unaligned(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        load_ty: nia_ids::InternedTyId,
        ptr: &FunctionExpr,
        span: Span,
    ) {
        self.current_subject = Some("unaligned load value");
        self.validate_runtime_type(load_ty, span);
        self.current_subject = None;
        self.validate_expr(ptr);
        if !self.same_type(result_ty, load_ty) {
            self.invalid_low_level_builtin(
                "unaligned load",
                span,
                "result type does not match its load metadata",
            );
        }
        let is_byte_pointer = match self.index.ty_kind(ptr.ty) {
            Some(TyKind::Pointer { elem, .. }) => matches!(
                self.index.ty_kind(*elem),
                Some(TyKind::Primitive(PrimitiveTy::U8))
            ),
            _ => false,
        };
        if !is_byte_pointer {
            self.invalid_low_level_builtin("unaligned load", span, "operand is not a byte pointer");
        }
    }

    fn validate_splat(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        value: &FunctionExpr,
        span: Span,
    ) {
        self.validate_expr(value);
        match self.index.ty_kind(result_ty) {
            Some(TyKind::Vector { elem, .. }) => {
                if !matches!(self.index.ty_kind(value.ty), Some(TyKind::Primitive(actual)) if actual == elem)
                {
                    self.invalid_low_level_builtin(
                        "SIMD splat",
                        span,
                        "scalar value type does not match the result lane type",
                    );
                }
            }
            _ => self.invalid_low_level_builtin("SIMD splat", span, "result is not a vector"),
        }
    }

    fn validate_vector_element(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        vector: &FunctionExpr,
        index: &FunctionExpr,
        inserted: Option<&FunctionExpr>,
        span: Span,
    ) {
        self.validate_expr(vector);
        self.validate_expr(index);
        if let Some(value) = inserted {
            self.validate_expr(value);
        }
        if !matches!(self.index.ty_kind(index.ty), Some(TyKind::Primitive(primitive)) if primitive.is_integer())
        {
            self.invalid_low_level_builtin(
                "SIMD lane",
                span,
                "index does not have an integer type",
            );
        }

        let Some(TyKind::Vector { elem, .. }) = self.index.ty_kind(vector.ty) else {
            self.invalid_low_level_builtin("SIMD lane", span, "operand is not a vector");
            return;
        };
        match inserted {
            Some(value) => {
                if !self.same_type(result_ty, vector.ty) {
                    self.invalid_low_level_builtin(
                        "SIMD insert",
                        span,
                        "result type does not match its vector operand",
                    );
                }
                if !matches!(self.index.ty_kind(value.ty), Some(TyKind::Primitive(actual)) if actual == elem)
                {
                    self.invalid_low_level_builtin(
                        "SIMD insert",
                        span,
                        "inserted value type does not match the vector lane type",
                    );
                }
            }
            None => {
                if !matches!(self.index.ty_kind(result_ty), Some(TyKind::Primitive(actual)) if actual == elem)
                {
                    self.invalid_low_level_builtin(
                        "SIMD extract",
                        span,
                        "result type does not match the vector lane type",
                    );
                }
            }
        }
    }

    fn validate_bitmask(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        vector: &FunctionExpr,
        span: Span,
    ) {
        self.validate_expr(vector);
        if !matches!(
            self.index.ty_kind(result_ty),
            Some(TyKind::Primitive(PrimitiveTy::Usize))
        ) {
            self.invalid_low_level_builtin("SIMD bitmask", span, "result type is not usize");
        }
        match self.index.ty_kind(vector.ty) {
            Some(TyKind::Vector {
                elem: PrimitiveTy::Bool,
                lanes,
            }) if u64::from(*lanes) <= self.target.pointer_size.saturating_mul(8) => {}
            Some(TyKind::Vector {
                elem: PrimitiveTy::Bool,
                ..
            }) => self.invalid_low_level_builtin(
                "SIMD bitmask",
                span,
                "mask exceeds the target usize width",
            ),
            _ => {
                self.invalid_low_level_builtin("SIMD bitmask", span, "operand is not a bool vector")
            }
        }
    }

    fn validate_bit_intrinsic(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        value: &FunctionExpr,
        span: Span,
    ) {
        self.validate_expr(value);
        if !matches!(self.index.ty_kind(value.ty), Some(TyKind::Primitive(primitive)) if primitive.is_integer())
        {
            self.invalid_low_level_builtin(
                "bit intrinsic",
                span,
                "operand does not have an integer type",
            );
        }
        if !self.same_type(result_ty, value.ty) {
            self.invalid_low_level_builtin(
                "bit intrinsic",
                span,
                "result type does not match its operand",
            );
        }
    }

    fn validate_char_from_u32(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        value: &FunctionExpr,
        span: Span,
    ) {
        self.validate_expr(value);
        if !matches!(
            self.index.ty_kind(value.ty),
            Some(TyKind::Primitive(PrimitiveTy::U32))
        ) {
            self.invalid_low_level_builtin("char conversion", span, "operand type is not u32");
        }
        let optional_char = match self.index.ty_kind(result_ty) {
            Some(TyKind::Optional { elem }) => matches!(
                self.index.ty_kind(*elem),
                Some(TyKind::Primitive(PrimitiveTy::Char))
            ),
            _ => false,
        };
        if !optional_char {
            self.invalid_low_level_builtin(
                "char conversion",
                span,
                "result type is not Optional[char]",
            );
        }
    }

    fn invalid_low_level_builtin(&mut self, kind: &'static str, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR {kind} has an invalid contract: {message}"),
        ));
    }

    fn validate_unary(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        op: UnaryOp,
        inner: &FunctionExpr,
        span: Span,
    ) {
        self.validate_expr(inner);
        match op {
            UnaryOp::Ref | UnaryOp::RefReadOnly => {
                if !matches!(
                    inner.kind,
                    FunctionExprKind::Function(_) | FunctionExprKind::FunctionInstance { .. }
                ) {
                    self.invalid_operator(
                        span,
                        "reference unary operation requires a function item",
                    );
                }
                if !matches!(
                    self.index.ty_kind(result_ty),
                    Some(TyKind::FunctionPointer { .. })
                ) {
                    self.invalid_operator(
                        span,
                        "function reference result is not a function pointer",
                    );
                }
                if !self.same_type(result_ty, inner.ty) {
                    self.invalid_operator(
                        span,
                        "function reference result type does not match its function item",
                    );
                }
            }
            UnaryOp::Deref => {
                let expected = match self.index.ty_kind(inner.ty) {
                    Some(TyKind::Pointer { elem, .. } | TyKind::VolatilePointer { elem, .. }) => {
                        Some(*elem)
                    }
                    _ => None,
                };
                if let Some(expected) = expected {
                    if !self.same_type(result_ty, expected) {
                        self.invalid_operator(span, "deref result type does not match its pointee");
                    }
                } else {
                    self.invalid_operator(span, "deref operand is not a pointer");
                }
            }
            UnaryOp::Neg => {
                if !self.is_numeric_operator_type(inner.ty) {
                    self.invalid_operator(span, "negation operand is not numeric");
                }
                if !self.same_type(result_ty, inner.ty) {
                    self.invalid_operator(span, "negation result type does not match its operand");
                }
            }
            UnaryOp::Not => {
                if !self.is_bool_type(inner.ty) {
                    self.invalid_operator(span, "logical not operand is not bool");
                }
                if !self.same_type(result_ty, inner.ty) {
                    self.invalid_operator(
                        span,
                        "logical not result type does not match its operand",
                    );
                }
            }
            UnaryOp::BitNot => {
                if !self.is_integer_operator_type(inner.ty) {
                    self.invalid_operator(span, "bitwise unary operand is not integer-like");
                }
                if !self.same_type(result_ty, inner.ty) {
                    self.invalid_operator(
                        span,
                        "bitwise unary result type does not match its operand",
                    );
                }
            }
        }
    }

    fn validate_cast(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        target_ty: nia_ids::InternedTyId,
        inner: &FunctionExpr,
        span: Span,
    ) {
        self.validate_expr(inner);
        self.current_subject = Some("cast target");
        self.validate_runtime_type(target_ty, span);
        self.current_subject = None;
        if !self.same_type(result_ty, target_ty) {
            self.invalid_operator(span, "cast result type does not match its target metadata");
        }
        if self.same_type(inner.ty, target_ty) {
            return;
        }

        let source_pointer = self.is_pointer_like_type(inner.ty);
        let target_pointer = self.is_pointer_like_type(target_ty);
        let source_pointer_int = self.is_pointer_integer_type(inner.ty);
        let target_pointer_int = self.is_pointer_integer_type(target_ty);
        let source_integer = self.is_cast_integer_type(inner.ty);
        let target_integer = self.is_cast_integer_type(target_ty);
        let source_float = self.is_cast_float_type(inner.ty);
        let target_float = self.is_cast_float_type(target_ty);
        let numeric = (source_integer || source_float) && (target_integer || target_float);
        let char_to_u32 = matches!(
            self.index.ty_kind(inner.ty),
            Some(TyKind::Primitive(PrimitiveTy::Char))
        ) && matches!(
            self.index.ty_kind(target_ty),
            Some(TyKind::Primitive(PrimitiveTy::U32))
        );
        let enum_cast = (self.is_enum_type(inner.ty) && target_integer)
            || (source_integer && self.is_enum_type(target_ty));
        let pointer_cast = (source_pointer && target_pointer)
            || (source_pointer && target_pointer_int)
            || (source_pointer_int && target_pointer);
        if !(numeric || char_to_u32 || enum_cast || pointer_cast) {
            self.invalid_operator(span, "cast source and target categories are incompatible");
            return;
        }
        if numeric && !self.cast_shapes_match(inner.ty, target_ty) {
            self.invalid_operator(span, "numeric cast changes scalar/vector shape");
        }
    }

    fn validate_tagged_union_constructor(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        inner: &FunctionExpr,
        constructor: TaggedUnionConstructor,
        span: Span,
    ) {
        self.validate_expr(inner);
        let expected_payload = match (constructor, self.index.ty_kind(result_ty)) {
            (TaggedUnionConstructor::OptionalSome, Some(TyKind::Optional { elem })) => Some(*elem),
            (TaggedUnionConstructor::ErrorOk, Some(TyKind::ErrorUnion { value, .. })) => {
                Some(*value)
            }
            (TaggedUnionConstructor::ErrorErr, Some(TyKind::ErrorUnion { error, .. })) => {
                Some(*error)
            }
            _ => None,
        };
        let Some(expected_payload) = expected_payload else {
            self.invalid_tagged_union(
                span,
                "constructor result is not the matching Optional or ErrorUnion type",
            );
            return;
        };
        if !self.same_type(inner.ty, expected_payload) {
            self.invalid_tagged_union(span, "constructor payload type does not match its result");
        }
    }

    fn validate_tagged_union_projection(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        inner: &FunctionExpr,
        projection: TaggedUnionProjection,
        span: Span,
    ) {
        self.validate_expr(inner);
        let Some(kind) = self.index.ty_kind(inner.ty) else {
            self.invalid_tagged_union(span, "projection input has no runtime type");
            return;
        };
        match (projection, kind) {
            (TaggedUnionProjection::Tag, TyKind::Optional { .. } | TyKind::ErrorUnion { .. }) => {
                if !matches!(
                    self.index.ty_kind(result_ty),
                    Some(TyKind::Primitive(PrimitiveTy::U8))
                ) {
                    self.invalid_tagged_union(span, "tag projection result is not u8");
                }
            }
            (TaggedUnionProjection::Payload, TyKind::Optional { elem }) => {
                if !self.same_type(result_ty, *elem) {
                    self.invalid_tagged_union(
                        span,
                        "optional payload result does not match its element",
                    );
                }
            }
            (TaggedUnionProjection::Payload, TyKind::ErrorUnion { error, value }) => {
                if !self.same_type(result_ty, *error) && !self.same_type(result_ty, *value) {
                    self.invalid_tagged_union(
                        span,
                        "error-union payload result matches neither error nor value type",
                    );
                }
            }
            (TaggedUnionProjection::Tag, _) => {
                self.invalid_tagged_union(span, "tag projection input is not a tagged union");
            }
            (TaggedUnionProjection::Payload, _) => {
                self.invalid_tagged_union(span, "payload projection input is not a tagged union");
            }
        }
    }

    fn invalid_tagged_union(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR tagged-union expression has an invalid contract: {message}"),
        ));
    }

    fn validate_enum_variant_expr(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        variant: nia_ids::GlobalDefId,
        fields: &[FunctionExpr],
        span: Span,
    ) {
        for field in fields {
            self.validate_expr(field);
        }
        let Some(info) = self.index.enum_variant_info(variant) else {
            self.validate_enum_variant_ref(
                variant,
                span,
                "backend IR expression references missing enum variant",
            );
            return;
        };
        self.validate_enum_variant_value(
            info.owner.backing_type,
            info.variant.value,
            info.index,
            span,
        );
        let owner = info.owner.def_id;
        let backing_type = info.owner.backing_type;
        let payload = info.variant.payload.clone();
        let Some(layout) = self.index.enum_layout(owner) else {
            self.invalid_enum(span, "variant owner has no enum layout");
            return;
        };
        // Fieldless enums use their backing integer directly; payload-bearing
        // enums use the nominal tagged aggregate. Derive that distinction from
        // the declared fields as well as the offset metadata so a scalar enum
        // remains valid when its layout has no payload fields.
        let has_payload = layout
            .variants
            .iter()
            .any(|variant| !variant.fields.is_empty());
        let result_matches = if has_payload {
            matches!(
                self.index.ty_kind(result_ty),
                Some(TyKind::Nominal { def_id, .. }) if *def_id == owner
            )
        } else {
            self.same_type(result_ty, backing_type)
                || matches!(
                    self.index.ty_kind(result_ty),
                    Some(TyKind::Nominal { def_id, .. }) if *def_id == owner
                )
        };
        if !result_matches {
            self.invalid_enum(
                span,
                "variant result type does not match its enum representation",
            );
        }

        let expected_fields = Self::enum_payload_types(&payload);
        if fields.len() != expected_fields.len() {
            self.invalid_enum(
                span,
                "variant payload field count does not match its declaration",
            );
        }
        for (field, expected_ty) in fields.iter().zip(expected_fields) {
            if !self.same_type(field.ty, expected_ty) {
                self.invalid_enum(
                    field.span,
                    "variant payload field type does not match its declaration",
                );
            }
        }
    }

    fn validate_enum_constructor(
        &mut self,
        function_pointer_ty: nia_ids::InternedTyId,
        variant: nia_ids::GlobalDefId,
        span: Span,
    ) {
        let Some(info) = self.index.enum_variant_info(variant) else {
            self.validate_enum_variant_ref(
                variant,
                span,
                "backend IR expression references missing enum constructor variant",
            );
            return;
        };
        let expected_params = match &info.variant.payload {
            nia_backend_ir::BackendEnumVariantPayload::Unit => {
                self.invalid_enum(
                    span,
                    "unit variant cannot be used as a constructor function",
                );
                return;
            }
            nia_backend_ir::BackendEnumVariantPayload::Tuple(fields) => fields.clone(),
            nia_backend_ir::BackendEnumVariantPayload::Named(fields) => {
                fields.iter().map(|field| field.ty).collect()
            }
        };
        let owner_id = info.owner.def_id;
        let Some(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        }) = self.ty_kind(function_pointer_ty).cloned()
        else {
            self.invalid_enum(span, "constructor value is not a function pointer");
            return;
        };
        if is_variadic {
            self.invalid_enum(span, "constructor function pointer is variadic");
        }
        if params.len() != expected_params.len()
            || !params
                .iter()
                .zip(expected_params)
                .all(|(actual, expected)| self.same_type(*actual, expected))
        {
            self.invalid_enum(
                span,
                "constructor function parameters do not match the variant payload",
            );
        }
        if !matches!(
            self.ty_kind(return_type),
            Some(TyKind::Nominal { def_id, .. }) if *def_id == owner_id
        ) {
            self.invalid_enum(
                span,
                "constructor function return type does not match the variant owner",
            );
        }
    }

    fn validate_enum_variant_tag(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        variant: nia_ids::GlobalDefId,
        span: Span,
    ) {
        let Some(info) = self.index.enum_variant_info(variant) else {
            self.validate_enum_variant_ref(
                variant,
                span,
                "backend IR expression references missing enum variant tag",
            );
            return;
        };
        self.validate_enum_variant_value(
            info.owner.backing_type,
            info.variant.value,
            info.index,
            span,
        );
        if !self.same_type(result_ty, info.owner.backing_type) {
            self.invalid_enum(
                span,
                "variant tag result does not match the enum backing type",
            );
        }
    }

    fn validate_enum_variant_value(
        &mut self,
        backing_type: nia_ids::InternedTyId,
        explicit_value: Option<i128>,
        index: usize,
        span: Span,
    ) {
        let value = explicit_value.unwrap_or(index as i128);
        let Some(TyKind::Primitive(primitive)) = self.ty_kind(backing_type) else {
            return;
        };
        let pointer_width = self
            .target
            .pointer_size
            .checked_mul(8)
            .and_then(|bits| u32::try_from(bits).ok())
            .unwrap_or(0);
        if !nia_ty::IntConst::from_i128(value).fits_primitive_int(*primitive, pointer_width) {
            self.invalid_enum(span, "variant value is outside its enum backing type");
        }
    }

    fn validate_enum_tag(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        value: &FunctionExpr,
        span: Span,
    ) {
        self.validate_expr(value);
        let expected_ty = match self.index.ty_kind(value.ty) {
            Some(TyKind::Nominal { def_id, .. }) => {
                let Some(item) = self.index.enum_item(*def_id) else {
                    self.invalid_enum(span, "tag input nominal type is not an enum");
                    return;
                };
                item.backing_type
            }
            Some(TyKind::Primitive(primitive)) if primitive.is_integer() => value.ty,
            _ => {
                self.invalid_enum(
                    span,
                    "tag input is not an enum value or integer representation",
                );
                return;
            }
        };
        if !self.same_type(result_ty, expected_ty) {
            self.invalid_enum(span, "tag result does not match the enum backing type");
        }
    }

    fn validate_enum_payload_field(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        value: &FunctionExpr,
        variant: nia_ids::GlobalDefId,
        field: usize,
        span: Span,
    ) {
        self.validate_expr(value);
        let Some(info) = self.index.enum_variant_info(variant) else {
            self.validate_enum_variant_ref(
                variant,
                span,
                "backend IR expression references missing enum payload variant",
            );
            return;
        };
        let owner = info.owner.def_id;
        let payload = info.variant.payload.clone();
        if !matches!(
            self.index.ty_kind(value.ty),
            Some(TyKind::Nominal { def_id, .. }) if *def_id == owner
        ) {
            self.invalid_enum(
                span,
                "payload projection input does not match the variant owner",
            );
        }
        if !self.index.enum_layout(owner).is_some_and(|layout| {
            layout
                .variants
                .iter()
                .any(|variant| !variant.fields.is_empty())
        }) {
            self.invalid_enum(span, "payload projection enum has no payload storage");
        }
        let fields = Self::enum_payload_types(&payload);
        let Some(expected_ty) = fields.get(field).copied() else {
            self.invalid_enum(span, "payload projection field index is out of bounds");
            return;
        };
        if !self.same_type(result_ty, expected_ty) {
            self.invalid_enum(
                span,
                "payload projection result does not match its field type",
            );
        }
    }

    fn enum_payload_types(
        payload: &nia_backend_ir::BackendEnumVariantPayload,
    ) -> Vec<nia_ids::InternedTyId> {
        match payload {
            nia_backend_ir::BackendEnumVariantPayload::Unit => Vec::new(),
            nia_backend_ir::BackendEnumVariantPayload::Tuple(fields) => fields.clone(),
            nia_backend_ir::BackendEnumVariantPayload::Named(fields) => {
                fields.iter().map(|field| field.ty).collect()
            }
        }
    }

    fn invalid_enum(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR enum expression has an invalid contract: {message}"),
        ));
    }

    fn is_cast_integer_type(&self, ty: nia_ids::InternedTyId) -> bool {
        match self.index.ty_kind(ty) {
            Some(TyKind::Primitive(
                PrimitiveTy::I8
                | PrimitiveTy::I16
                | PrimitiveTy::I32
                | PrimitiveTy::I64
                | PrimitiveTy::I128
                | PrimitiveTy::Isize
                | PrimitiveTy::U8
                | PrimitiveTy::U16
                | PrimitiveTy::U32
                | PrimitiveTy::U64
                | PrimitiveTy::U128
                | PrimitiveTy::Usize,
            )) => true,
            Some(TyKind::Vector { elem, .. }) => matches!(
                elem,
                PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::I128
                    | PrimitiveTy::Isize
                    | PrimitiveTy::U8
                    | PrimitiveTy::U16
                    | PrimitiveTy::U32
                    | PrimitiveTy::U64
                    | PrimitiveTy::U128
                    | PrimitiveTy::Usize
                    | PrimitiveTy::Bool
            ),
            _ => false,
        }
    }

    fn is_cast_float_type(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(
            self.index.ty_kind(ty),
            Some(TyKind::Primitive(PrimitiveTy::F32 | PrimitiveTy::F64))
                | Some(TyKind::Vector {
                    elem: PrimitiveTy::F32 | PrimitiveTy::F64,
                    ..
                })
        )
    }

    fn is_pointer_like_type(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(
            self.index.ty_kind(ty),
            Some(
                TyKind::Pointer { .. }
                    | TyKind::VolatilePointer { .. }
                    | TyKind::FunctionPointer { .. }
            )
        )
    }

    fn is_pointer_integer_type(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(
            self.index.ty_kind(ty),
            Some(TyKind::Primitive(PrimitiveTy::Isize | PrimitiveTy::Usize))
        )
    }

    fn is_enum_type(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(
            self.index.ty_kind(ty),
            Some(TyKind::Nominal { def_id, .. }) if self.index.has_enum(*def_id)
        )
    }

    fn cast_shapes_match(
        &self,
        source: nia_ids::InternedTyId,
        target: nia_ids::InternedTyId,
    ) -> bool {
        match (self.index.ty_kind(source), self.index.ty_kind(target)) {
            (
                Some(TyKind::Vector { lanes: source, .. }),
                Some(TyKind::Vector { lanes: target, .. }),
            ) => source == target,
            (Some(TyKind::Vector { .. }), _) | (_, Some(TyKind::Vector { .. })) => false,
            _ => true,
        }
    }

    fn validate_binary(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        lhs: &FunctionExpr,
        op: BinaryOp,
        rhs: &FunctionExpr,
        span: Span,
    ) {
        self.validate_expr(lhs);
        self.validate_expr(rhs);
        self.validate_binary_contract(result_ty, lhs.ty, op, rhs.ty, span);
    }

    fn validate_binary_contract(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        lhs_ty: nia_ids::InternedTyId,
        op: BinaryOp,
        rhs_ty: nia_ids::InternedTyId,
        span: Span,
    ) {
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            if !self.is_bool_type(lhs_ty)
                || !self.is_bool_type(rhs_ty)
                || !self.is_bool_type(result_ty)
            {
                self.invalid_operator(span, "logical operator requires bool operands and result");
            }
            return;
        }

        let matching_operands = if matches!(op, BinaryOp::Shl | BinaryOp::Shr) {
            match self.index.ty_kind(lhs_ty) {
                Some(TyKind::Vector { .. }) => self.same_type(lhs_ty, rhs_ty),
                _ => self.is_integer_operator_type(rhs_ty),
            }
        } else {
            self.same_type(lhs_ty, rhs_ty)
        };
        if !matching_operands {
            self.invalid_operator(span, "binary operands do not have a compatible type");
        }

        let valid_operand = match op {
            BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::Rem
            | BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge => self.is_numeric_operator_type(lhs_ty) || self.is_char_type(lhs_ty),
            BinaryOp::Eq | BinaryOp::Ne => self.is_comparable_operator_type(lhs_ty),
            BinaryOp::BitAnd | BinaryOp::BitXor | BinaryOp::BitOr => {
                self.is_bitwise_operator_type(lhs_ty)
            }
            BinaryOp::Shl | BinaryOp::Shr => self.is_integer_operator_type(lhs_ty),
            BinaryOp::And | BinaryOp::Or => true,
        };
        if !valid_operand {
            self.invalid_operator(
                span,
                "binary operand type is not supported by the operation",
            );
        }

        let comparison = matches!(
            op,
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Eq | BinaryOp::Ne
        );
        let expected_result = if comparison {
            match self.index.ty_kind(lhs_ty) {
                Some(TyKind::Vector { lanes, .. }) => self
                    .index
                    .ty_kind(result_ty)
                    .is_some_and(|kind| matches!(kind, TyKind::Vector { elem: PrimitiveTy::Bool, lanes: result_lanes } if result_lanes == lanes)),
                _ => self.is_bool_type(result_ty),
            }
        } else {
            self.same_type(result_ty, lhs_ty)
        };
        if !expected_result {
            self.invalid_operator(span, "binary result type does not match the operation");
        }
    }

    fn validate_callable_coercion(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        state: &FunctionExpr,
        closure_id: nia_ids::ClosureId,
        span: Span,
    ) {
        self.validate_expr(state);
        let Some(TyKind::Callable {
            is_readonly: callable_readonly,
            params: callable_params,
            return_type: callable_return,
        }) = self.ty_kind(result_ty).cloned()
        else {
            self.invalid_callable_coercion(span, "result is not callable");
            return;
        };
        let Some(TyKind::Pointer {
            is_readonly: state_readonly,
            elem: state_ty,
        }) = self.ty_kind(state.ty).cloned()
        else {
            self.invalid_callable_coercion(span, "state is not a pointer");
            return;
        };
        let Some(TyKind::ClosureState {
            closure_id: state_closure_id,
            params: state_params,
            return_type: state_return,
            ..
        }) = self.ty_kind(state_ty).cloned()
        else {
            self.invalid_callable_coercion(span, "state pointer does not target closure state");
            return;
        };
        if state_closure_id != closure_id {
            self.invalid_callable_coercion(span, "closure identity does not match its state");
        }
        if !callable_readonly && state_readonly {
            self.invalid_callable_coercion(span, "mutable callable has a readonly state pointer");
        }
        if !self.same_type_args(&callable_params, &state_params)
            || !self.same_type(callable_return, state_return)
        {
            self.invalid_callable_coercion(span, "callable signature does not match closure state");
        }
        let Some(entry) = self.current_closure_entry(closure_id) else {
            self.invalid_callable_coercion(span, "generated closure entry is missing");
            return;
        };
        if !self.same_type(entry.abi.state_type, state_ty)
            || !self.same_type_args(&entry.abi.params, &callable_params)
            || !self.same_type(entry.abi.return_type, callable_return)
        {
            self.invalid_callable_coercion(span, "generated entry ABI does not match the callable");
        }
    }

    fn validate_closure_function_pointer(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        closure_id: nia_ids::ClosureId,
        span: Span,
    ) {
        let Some(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic: false,
        }) = self.ty_kind(result_ty).cloned()
        else {
            self.invalid_callable_coercion(
                span,
                "closure function-pointer result is not a non-variadic function pointer",
            );
            return;
        };
        let Some(entry) = self.current_closure_entry(closure_id) else {
            self.invalid_callable_coercion(span, "generated closure entry is missing");
            return;
        };
        let state_contract = match self.ty_kind(entry.abi.state_type) {
            Some(TyKind::ClosureState {
                closure_id: state_closure_id,
                captures,
                params: state_params,
                return_type: state_return,
            }) => {
                *state_closure_id == closure_id
                    && captures.is_empty()
                    && self.same_type_args(state_params, &params)
                    && self.same_type(*state_return, return_type)
            }
            _ => false,
        };
        if !state_contract
            || !self.same_type_args(&entry.abi.params, &params)
            || !self.same_type(entry.abi.return_type, return_type)
        {
            self.invalid_callable_coercion(
                span,
                "closure entry is capturing or has a mismatched function-pointer ABI",
            );
        }
    }

    fn current_closure_entry(
        &self,
        closure_id: nia_ids::ClosureId,
    ) -> Option<&nia_backend_ir::BackendClosureEntry> {
        let owner = self.current_closure_owner.clone()?;
        self.index
            .closure_entry(&nia_backend_ir::BackendClosureEntryKey { closure_id, owner })
    }

    fn invalid_callable_coercion(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR callable coercion has an invalid contract: {message}"),
        ));
    }

    fn validate_trait_object_upcast(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        inner: &FunctionExpr,
        source_ty: nia_ids::InternedTyId,
        target_ty: nia_ids::InternedTyId,
        span: Span,
    ) {
        self.validate_expr(inner);
        self.validate_runtime_type(source_ty, span);
        self.validate_runtime_type(target_ty, span);
        let source_readonly = match self.ty_kind(source_ty) {
            Some(TyKind::TraitObject { is_readonly, .. }) => Some(*is_readonly),
            _ => None,
        };
        let target_readonly = match self.ty_kind(target_ty) {
            Some(TyKind::TraitObject { is_readonly, .. }) => Some(*is_readonly),
            _ => None,
        };
        if source_readonly.is_none() || target_readonly.is_none() {
            self.invalid_trait_object(span, "upcast source and target must be trait objects");
            return;
        }
        if !self.same_type(inner.ty, source_ty) {
            self.invalid_trait_object(span, "upcast source metadata does not match the operand");
        }
        if !self.same_type(result_ty, target_ty) {
            self.invalid_trait_object(span, "upcast result type does not match target metadata");
        }
        if source_readonly == Some(true) && target_readonly == Some(false) {
            self.invalid_trait_object(span, "upcast cannot strengthen readonly access");
        }
    }

    fn validate_trait_object_coercion(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        inner: &FunctionExpr,
        self_ty: nia_ids::InternedTyId,
        target_ty: nia_ids::InternedTyId,
        span: Span,
    ) {
        self.validate_expr(inner);
        // `self_ty` may be an unsized pointee marker (for example
        // `SlicePointee`), so validate its recursive type identity without
        // requiring a standalone ABI layout.
        self.validate_type(self_ty, span);
        self.validate_runtime_type(target_ty, span);
        let Some(target_readonly) = (match self.ty_kind(target_ty) {
            Some(TyKind::TraitObject { is_readonly, .. }) => Some(*is_readonly),
            _ => None,
        }) else {
            self.invalid_trait_object(span, "coercion target is not a trait object");
            return;
        };
        if !self.same_type(result_ty, target_ty) {
            self.invalid_trait_object(span, "coercion result type does not match target metadata");
        }
        let source = match self.ty_kind(inner.ty) {
            Some(TyKind::Pointer { is_readonly, elem }) => Some((*is_readonly, *elem)),
            Some(TyKind::Slice { is_readonly, elem }) => Some((*is_readonly, *elem)),
            _ => None,
        };
        let Some((source_readonly, source_elem)) = source else {
            self.invalid_trait_object(span, "coercion source is not a pointer or slice");
            return;
        };
        let source_matches_self = self.same_type(source_elem, self_ty)
            || match self.ty_kind(self_ty) {
                Some(TyKind::SlicePointee { elem }) => self.same_type(source_elem, *elem),
                _ => false,
            };
        if !source_matches_self {
            self.invalid_trait_object(span, "coercion self type does not match source element");
        }
        if !target_readonly && source_readonly {
            self.invalid_trait_object(span, "coercion cannot strengthen readonly access");
        }
        let key = nia_backend_ir::BackendTraitObjectVtableKey {
            self_ty,
            object_ty: target_ty,
        };
        if self.index.trait_object_vtable(&key).is_none() {
            self.invalid_trait_object(span, "coercion target vtable is missing");
        }
    }

    fn invalid_trait_object(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR trait-object expression has an invalid contract: {message}"),
        ));
    }

    fn is_bool_type(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(
            self.index.ty_kind(ty),
            Some(TyKind::Primitive(PrimitiveTy::Bool))
        )
    }

    fn is_char_type(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(
            self.index.ty_kind(ty),
            Some(TyKind::Primitive(PrimitiveTy::Char))
        )
    }

    fn is_comparable_operator_type(&self, ty: nia_ids::InternedTyId) -> bool {
        self.is_numeric_operator_type(ty)
            || self.is_bool_type(ty)
            || self.is_char_type(ty)
            || match self.index.ty_kind(ty) {
                Some(TyKind::Nominal { def_id, .. }) => self.index.has_enum(*def_id),
                Some(TyKind::Pointer { .. } | TyKind::FunctionPointer { .. }) => true,
                _ => false,
            }
    }

    fn is_integer_operator_type(&self, ty: nia_ids::InternedTyId) -> bool {
        match self.index.ty_kind(ty) {
            Some(TyKind::Primitive(primitive)) => primitive.is_integer(),
            Some(TyKind::Nominal { def_id, .. }) => self.index.has_enum(*def_id),
            Some(TyKind::Vector { elem, .. }) => elem.is_integer(),
            _ => false,
        }
    }

    fn is_bitwise_operator_type(&self, ty: nia_ids::InternedTyId) -> bool {
        self.is_integer_operator_type(ty)
            || matches!(
                self.index.ty_kind(ty),
                Some(TyKind::Vector {
                    elem: PrimitiveTy::Bool,
                    ..
                })
            )
    }

    fn is_numeric_operator_type(&self, ty: nia_ids::InternedTyId) -> bool {
        self.is_integer_operator_type(ty)
            || matches!(
                self.index.ty_kind(ty),
                Some(TyKind::Primitive(PrimitiveTy::F32 | PrimitiveTy::F64))
                    | Some(TyKind::Vector {
                        elem: PrimitiveTy::F32 | PrimitiveTy::F64,
                        ..
                    })
            )
    }

    fn invalid_operator(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR operator has an invalid contract: {message}"),
        ));
    }

    fn validate_projection_result_type(
        &mut self,
        actual_ty: nia_ids::InternedTyId,
        expected_ty: nia_ids::InternedTyId,
        span: Span,
        kind: &'static str,
    ) {
        if !self.projection_result_compatible(actual_ty, expected_ty) {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!("backend IR {kind} result type does not match its selected value"),
            ));
        }
    }

    fn projection_result_compatible(
        &self,
        actual_ty: nia_ids::InternedTyId,
        selected_ty: nia_ids::InternedTyId,
    ) -> bool {
        if self.same_type(actual_ty, selected_ty) {
            return true;
        }
        // Expected-type coercion is represented directly on place expressions,
        // without a separate Function IR node. Mirror the front-end's only
        // permitted qualifier coercion: a mutable selected value may be viewed
        // as readonly, while readonly-to-mutable projection remains invalid.
        match (
            self.index.ty_kind(actual_ty),
            self.index.ty_kind(selected_ty),
        ) {
            (
                Some(TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }),
                Some(TyKind::Pointer {
                    is_readonly: selected_readonly,
                    elem: selected_elem,
                }),
            )
            | (
                Some(TyKind::VolatilePointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }),
                Some(TyKind::VolatilePointer {
                    is_readonly: selected_readonly,
                    elem: selected_elem,
                }),
            )
            | (
                Some(TyKind::Slice {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }),
                Some(TyKind::Slice {
                    is_readonly: selected_readonly,
                    elem: selected_elem,
                }),
            ) => {
                (*actual_readonly || !*selected_readonly)
                    && self.projection_result_compatible(*actual_elem, *selected_elem)
            }
            _ => false,
        }
    }

    fn invalid_global_value_type(&mut self, span: Span, kind: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR {kind} expression type does not match its storage type"),
        ));
    }

    fn validate_slice_contract(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        lhs: &FunctionExpr,
        requested_readonly: bool,
        span: Span,
    ) {
        self.validate_expr(lhs);
        let Some(source) = self.slice_source_info(lhs.ty) else {
            self.invalid_slice(span, "slice input is not an array, pointer, or slice");
            return;
        };
        let Some(TyKind::Slice {
            is_readonly,
            elem: result_elem,
        }) = self.index.ty_kind(result_ty)
        else {
            self.invalid_slice(span, "slice result is not a Slice type");
            return;
        };
        if !self.same_type(*result_elem, source.elem) {
            self.invalid_slice(span, "slice result element does not match its input");
        }
        if *is_readonly != requested_readonly {
            self.invalid_slice(
                span,
                "slice result readonly metadata does not match the expression",
            );
        }
        if source.readonly && !*is_readonly {
            self.invalid_slice(span, "slice drops readonly access from its input");
        }
    }

    fn validate_slice_bound(&mut self, bound: &FunctionExpr) {
        if !self.is_integer_type(bound.ty) {
            self.invalid_slice(bound.span, "slice range bound is not an integer");
        }
    }

    fn slice_source_info(&self, ty: nia_ids::InternedTyId) -> Option<SliceSourceInfo> {
        match self.index.ty_kind(ty) {
            Some(TyKind::Array { elem, .. }) => Some(SliceSourceInfo {
                elem: *elem,
                readonly: false,
            }),
            Some(TyKind::Pointer { is_readonly, elem })
            | Some(TyKind::VolatilePointer { is_readonly, elem }) => {
                if let Some(TyKind::Array {
                    elem: array_elem, ..
                }) = self.index.ty_kind(*elem)
                {
                    Some(SliceSourceInfo {
                        elem: *array_elem,
                        readonly: *is_readonly,
                    })
                } else {
                    Some(SliceSourceInfo {
                        elem: *elem,
                        readonly: *is_readonly,
                    })
                }
            }
            Some(TyKind::Slice { is_readonly, elem }) => Some(SliceSourceInfo {
                elem: *elem,
                readonly: *is_readonly,
            }),
            _ => None,
        }
    }

    fn invalid_slice(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR slice expression has an invalid contract: {message}"),
        ));
    }

    fn has_direct_aggregate_field_contract(&self, ty: nia_ids::InternedTyId) -> bool {
        // Generic declarations can be validated before their concrete instance
        // fields are published, in which case aggregate lookup intentionally
        // falls back to symbolic declaration fields. Enforce value-type equality
        // for monomorphic declarations and published concrete instances; field
        // identity is still validated for symbolic generic aggregates above.
        let Some((def_id, args, const_args)) = self.field_base_type(ty) else {
            return false;
        };
        if args.is_empty() && const_args.is_empty() {
            return true;
        }
        self.index
            .struct_instance(def_id, &args, &const_args)
            .is_some()
            || self
                .index
                .union_instance(def_id, &args, &const_args)
                .is_some()
            || self.index.struct_instances_for(def_id).any(|item| {
                self.same_type_args(&item.args, &args)
                    && self.same_const_args(&item.const_args, &const_args)
            })
            || self.index.union_instances_for(def_id).any(|item| {
                self.same_type_args(&item.args, &args)
                    && self.same_const_args(&item.const_args, &const_args)
            })
    }

    fn validate_aggregate_literal_identity(
        &mut self,
        kind: &'static str,
        result_ty: nia_ids::InternedTyId,
        def_id: nia_ids::GlobalDefId,
        span: Span,
    ) {
        if !matches!(
            self.ty_kind(result_ty),
            Some(TyKind::Nominal { def_id: result_def, .. }) if *result_def == def_id
        ) {
            self.invalid_literal_contract(span, kind, "definition does not match expression type");
        }
        let valid_kind = match kind {
            "struct" => self.index.has_struct(def_id) || self.index.has_struct_instances(def_id),
            "union" => self.index.has_union(def_id) || self.index.has_union_instances(def_id),
            _ => false,
        };
        if !valid_kind {
            self.invalid_literal_contract(span, kind, "definition has the wrong aggregate kind");
        }
    }

    fn validate_struct_literal_field_coverage(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        fields: &[nia_function_ir::FunctionFieldInit],
        span: Span,
    ) {
        let Some((def_id, args, const_args)) = self.field_base_type(result_ty) else {
            return;
        };
        let Some(expected) = self.aggregate_fields(def_id, &args, &const_args) else {
            return;
        };
        // LLVM literal emission stores only the supplied fields and then loads
        // the complete alloca. Requiring a set equality here prevents missing
        // or duplicate logical fields from exposing uninitialized bytes.
        let supplied = fields
            .iter()
            .filter_map(|field| field.field)
            .collect::<HashSet<_>>();
        let covers_exactly = supplied.len() == fields.len()
            && supplied.len() == expected.len()
            && supplied
                .iter()
                .all(|field| expected.iter().any(|candidate| candidate.def_id == *field));
        if !covers_exactly {
            self.invalid_literal_contract(
                span,
                "struct",
                "fields do not initialize each declared field exactly once",
            );
        }
    }

    fn invalid_literal_contract(&mut self, span: Span, kind: &'static str, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR {kind} literal has an invalid type contract: {message}"),
        ));
    }

    fn validate_integer_literal(&mut self, ty: nia_ids::InternedTyId, text: &str, span: Span) {
        let Some(TyKind::Primitive(primitive)) = self.index.ty_kind(ty) else {
            self.invalid_literal_contract(span, "integer", "target type is not an integer");
            return;
        };
        let primitive = *primitive;
        if !primitive.is_integer() && !matches!(primitive, PrimitiveTy::Bool | PrimitiveTy::Char) {
            self.invalid_literal_contract(span, "integer", "target type is not an integer");
            return;
        }
        let Some(value) = parse_int_literal(text) else {
            self.invalid_literal_contract(span, "integer", "spelling is invalid");
            return;
        };
        if !self.integer_literal_fits(primitive, value) {
            self.invalid_literal_contract(span, "integer", "value is outside its target type");
        }
    }

    /// Applies source integer ranges before LLVM's constant constructors can
    /// truncate a malformed backend value to the destination bit width.
    fn integer_literal_fits(&self, primitive: PrimitiveTy, value: nia_ty::IntConst) -> bool {
        match primitive {
            PrimitiveTy::Bool => !value.is_signed() && matches!(value.bits(), 0 | 1),
            PrimitiveTy::Char => {
                !value.is_signed()
                    && u32::try_from(value.bits())
                        .ok()
                        .and_then(char::from_u32)
                        .is_some()
            }
            primitive if primitive.is_integer() => value.fits_primitive_int(
                primitive,
                self.target
                    .pointer_size
                    .checked_mul(8)
                    .and_then(|bits| u32::try_from(bits).ok())
                    .unwrap_or(0),
            ),
            _ => false,
        }
    }

    fn validate_float_literal(&mut self, ty: nia_ids::InternedTyId, text: &str, span: Span) {
        let primitive = match self.index.ty_kind(ty) {
            Some(TyKind::Primitive(primitive @ (PrimitiveTy::F32 | PrimitiveTy::F64))) => {
                *primitive
            }
            _ => {
                self.invalid_literal_contract(span, "float", "target type is not f32 or f64");
                return;
            }
        };
        let Some(value) = parse_float_literal(text) else {
            self.invalid_literal_contract(span, "float", "spelling is invalid");
            return;
        };
        if !value.is_finite()
            || matches!(primitive, PrimitiveTy::F32) && !(value as f32).is_finite()
        {
            self.invalid_literal_contract(span, "float", "value is outside its target type");
        }
    }

    fn validate_string_scalars(&mut self, scalars: &[u32], span: Span) {
        if scalars
            .iter()
            .any(|scalar| char::from_u32(*scalar).is_none())
        {
            self.invalid_literal_contract(
                span,
                "string",
                "value contains an invalid Unicode scalar",
            );
        }
    }

    fn validate_string_literal(
        &mut self,
        ty: nia_ids::InternedTyId,
        length: usize,
        span: Span,
        bytes: bool,
    ) {
        let kind = if bytes { "byte string" } else { "string" };
        let expected_elem = if bytes {
            PrimitiveTy::U8
        } else {
            PrimitiveTy::Char
        };
        let Some(TyKind::Array { len, elem }) = self.index.ty_kind(ty).cloned() else {
            self.invalid_literal_contract(span, kind, "target type is not an array");
            return;
        };
        if !matches!(self.index.ty_kind(elem), Some(TyKind::Primitive(actual)) if *actual == expected_elem)
        {
            self.invalid_literal_contract(span, kind, "array element type does not match literal");
        }
        if self
            .array_len_value(&len)
            .is_some_and(|expected| u64::try_from(length) != Ok(expected))
        {
            self.invalid_literal_contract(span, kind, "array length does not match literal");
        }
    }

    fn validate_range_expr(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        range: &nia_function_ir::FunctionRange,
        span: Span,
    ) {
        for value in range.start.iter().chain(range.end.iter()) {
            self.validate_expr(value);
        }
        let Some(TyKind::Range { kind, bound }) = self.index.ty_kind(result_ty).cloned() else {
            self.invalid_range(span, "expression type is not a range");
            return;
        };
        let has_start = range.start.is_some();
        let has_end = range.end.is_some();
        if has_start != kind.has_start_bound() || has_end != kind.has_end_bound() {
            self.invalid_range(span, "range bound presence does not match its range kind");
        }
        let expected_inclusive = matches!(
            kind,
            nia_ty::RangeTyKind::Inclusive | nia_ty::RangeTyKind::ToInclusive
        );
        if range.inclusive != expected_inclusive {
            self.invalid_range(span, "inclusive metadata does not match its range kind");
        }
        let Some(bound_ty) = bound else {
            if has_start || has_end {
                self.invalid_range(span, "full range carries a bound expression");
            }
            return;
        };
        for value in range.start.iter().chain(range.end.iter()) {
            if !self.same_type(value.ty, bound_ty) {
                self.invalid_range(span, "range bound type does not match its range bound type");
            }
        }
    }

    fn validate_static_array_pointer(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        array: &FunctionExpr,
        declared_readonly: bool,
        span: Span,
    ) {
        self.validate_expr(array);
        if !matches!(self.index.ty_kind(array.ty), Some(TyKind::Array { .. })) {
            self.invalid_static_array_pointer(span, "promoted value is not an array");
        }
        let Some(TyKind::Pointer { is_readonly, elem }) = self.index.ty_kind(result_ty) else {
            self.invalid_static_array_pointer(span, "result type is not a pointer");
            return;
        };
        if *is_readonly != declared_readonly {
            self.invalid_static_array_pointer(span, "readonly metadata does not match its result");
        }
        if !self.same_type(*elem, array.ty) {
            self.invalid_static_array_pointer(
                span,
                "result pointer element does not match the promoted array",
            );
        }
    }

    fn invalid_static_array_pointer(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR static array pointer has an invalid contract: {message}"),
        ));
    }

    fn validate_range_bound(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        range: &FunctionExpr,
        bound: nia_function_ir::FunctionRangeBound,
        span: Span,
    ) {
        self.validate_expr(range);
        let Some(TyKind::Range {
            kind,
            bound: Some(bound_ty),
        }) = self.index.ty_kind(range.ty).cloned()
        else {
            self.invalid_range(span, "bound projection input is not a bounded range");
            return;
        };
        let available = match bound {
            nia_function_ir::FunctionRangeBound::Start => kind.has_start_bound(),
            nia_function_ir::FunctionRangeBound::End => kind.has_end_bound(),
        };
        if !available {
            self.invalid_range(span, "requested bound is not present for the range kind");
        }
        if !self.same_type(result_ty, bound_ty) {
            self.invalid_range(
                span,
                "bound projection result does not match its range bound type",
            );
        }
    }

    fn invalid_range(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR range expression has an invalid contract: {message}"),
        ));
    }

    fn validate_builtin_value(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        value: &nia_function_ir::FunctionBuiltinValue,
        span: Span,
    ) {
        use nia_function_ir::FunctionBuiltinValue;

        match value {
            FunctionBuiltinValue::Usize(value) => {
                self.validate_builtin_usize_result(result_ty, span);
                if !self.builtin_usize_fits(*value) {
                    self.invalid_builtin_value(
                        span,
                        "usize constant value is outside its target pointer width",
                    );
                }
            }
            FunctionBuiltinValue::Layout { ty, .. } => {
                self.current_subject = Some("layout builtin operand");
                self.validate_runtime_type(*ty, span);
                self.current_subject = None;
                self.validate_builtin_usize_result(result_ty, span);
            }
            FunctionBuiltinValue::FieldOffset { ty, field } => {
                self.current_subject = Some("field-offset builtin operand");
                self.validate_runtime_type(*ty, span);
                self.current_subject = None;
                self.validate_aggregate_field(
                    *ty,
                    *field,
                    span,
                    "backend IR field-offset builtin references missing field",
                );
                self.validate_builtin_usize_result(result_ty, span);
            }
            FunctionBuiltinValue::Int(value) => {
                if !self.is_integer_type(result_ty) {
                    self.invalid_builtin_value(span, "integer constant result is not integer-like");
                } else if !self.builtin_integer_fits(result_ty, *value) {
                    self.invalid_builtin_value(
                        span,
                        "integer constant value is outside its result type",
                    );
                }
            }
        }
    }

    fn validate_builtin_usize_result(&mut self, result_ty: nia_ids::InternedTyId, span: Span) {
        if !matches!(
            self.index.ty_kind(result_ty),
            Some(TyKind::Primitive(PrimitiveTy::Usize))
        ) {
            self.invalid_builtin_value(span, "result type is not usize");
        }
    }

    fn builtin_usize_fits(&self, value: u64) -> bool {
        let Some(bits) = self
            .target
            .pointer_size
            .checked_mul(8)
            .and_then(|bits| u32::try_from(bits).ok())
        else {
            return false;
        };
        bits >= u64::BITS || value < (1_u64 << bits)
    }

    fn builtin_integer_fits(&self, ty: nia_ids::InternedTyId, value: nia_ty::IntConst) -> bool {
        let Some(TyKind::Primitive(primitive)) = self.ty_kind(ty) else {
            return false;
        };
        let pointer_width = self
            .target
            .pointer_size
            .checked_mul(8)
            .and_then(|bits| u32::try_from(bits).ok())
            .unwrap_or(0);
        match *primitive {
            PrimitiveTy::Bool => !value.is_signed() && matches!(value.bits(), 0 | 1),
            PrimitiveTy::Char => {
                !value.is_signed()
                    && u32::try_from(value.bits())
                        .ok()
                        .and_then(char::from_u32)
                        .is_some()
            }
            primitive if primitive.is_integer() => {
                value.fits_primitive_int(primitive, pointer_width)
                    || (value.is_signed()
                        && primitive.is_signed_integer()
                        && primitive.integer_bits(pointer_width).is_some_and(|bits| {
                            bits < u128::BITS
                                && value.bits() < (1_u128 << bits)
                                && value.bits() >= (1_u128 << (bits - 1))
                        }))
            }
            _ => false,
        }
    }

    fn invalid_builtin_value(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR builtin value has an invalid contract: {message}"),
        ));
    }

    fn validate_local_type(
        &mut self,
        local_id: nia_ids::LocalId,
        actual_ty: nia_ids::InternedTyId,
        span: Span,
        message: &'static str,
    ) {
        let Some(expected_ty) = self
            .local_tys
            .last()
            .and_then(|locals| locals.get(&local_id))
            .copied()
        else {
            return;
        };
        if !self.same_type(expected_ty, actual_ty) {
            self.invalid_local_type(span, message);
        }
    }

    fn invalid_local_type(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR contains an invalid local type contract: {message}"),
        ));
    }
}
