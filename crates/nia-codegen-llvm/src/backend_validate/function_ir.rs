// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashSet;

use nia_ast::{AssignOp, BinaryOp, UnaryOp};
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    AtomicOrder, AtomicRmwOp, FunctionArrayElements, FunctionBody, FunctionCallee,
    FunctionDeferBody, FunctionExpr, FunctionExprKind, FunctionOp, FunctionPlace,
    FunctionPlaceBase, FunctionPlaceElem, FunctionTerminator, FunctionTryKind,
    validate_function_body,
};
use nia_mangle::mangle_symbol_id;
use nia_span::Span;
use nia_ty::{ConstGenericArg, ConstGenericValue, PrimitiveTy, TyKind};

use crate::literals::{
    assign_to_binary_op, decode_byte_char_literal, parse_float_literal, parse_int_literal,
};

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
    span: Span,
}

#[derive(Clone, Copy)]
struct VtableTraitInstance<'a> {
    trait_id: nia_ty::TraitId,
    args: &'a [nia_ids::InternedTyId],
    const_args: &'a [ConstGenericArg],
}

struct CallTargetSignature {
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
            FunctionExprKind::Integer(text) => parse_int_literal(text)? as u128,
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
                if self.index.module(allocation.module_id()).is_none() {
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
                    let Some(owner) = self.index.module(relocation.allocation.module_id()) else {
                        self.diagnostics.push(Diagnostic::internal_error_at(
                            nia_diagnostic::codes::INVALID_BACKEND_IR,
                            expr.span,
                            "backend IR union storage relocation references a missing module",
                        ));
                        continue;
                    };
                    if usize::try_from(owner.layouts.target.pointer_size).ok()
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
                    Some(TyKind::Pointer { .. } | TyKind::FunctionPointer { .. })
                ) {
                    self.invalid_operator(span, "function reference result is not pointer-like");
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
        if !self.same_type(result_ty, info.owner.backing_type) {
            self.invalid_enum(
                span,
                "variant tag result does not match the enum backing type",
            );
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
                self.is_integer_operator_type(lhs_ty)
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
            || self
                .index
                .struct_instances_for(def_id)
                .any(|item| self.same_type_args(&item.args, &args) && item.const_args == const_args)
            || self
                .index
                .union_instances_for(def_id)
                .any(|item| self.same_type_args(&item.args, &args) && item.const_args == const_args)
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
            _ => unreachable!("aggregate literal kind is statically selected"),
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
    fn integer_literal_fits(&self, primitive: PrimitiveTy, value: i128) -> bool {
        match primitive {
            PrimitiveTy::Bool => matches!(value, 0 | 1),
            PrimitiveTy::Char => u32::try_from(value).ok().and_then(char::from_u32).is_some(),
            primitive if primitive.is_signed_integer() => {
                let Some(bits) = self.primitive_integer_bits(primitive) else {
                    return false;
                };
                bits == i128::BITS
                    || ((-(1_i128 << (bits - 1)))..=((1_i128 << (bits - 1)) - 1)).contains(&value)
            }
            primitive if primitive.is_integer() => {
                let Some(bits) = self.primitive_integer_bits(primitive) else {
                    return false;
                };
                value >= 0 && (bits == u128::BITS || (value as u128) < (1_u128 << bits))
            }
            _ => false,
        }
    }

    fn primitive_integer_bits(&self, primitive: PrimitiveTy) -> Option<u32> {
        match primitive {
            PrimitiveTy::Isize | PrimitiveTy::Usize => self
                .target
                .pointer_size
                .checked_mul(8)
                .and_then(|bits| u32::try_from(bits).ok()),
            _ => primitive.integer_bits(0),
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
            FunctionBuiltinValue::Usize(_) => {
                self.validate_builtin_usize_result(result_ty, span);
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
            FunctionBuiltinValue::Int(_) => {
                if !self.is_integer_type(result_ty) {
                    self.invalid_builtin_value(span, "integer constant result is not integer-like");
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

    fn validate_atomic(
        &mut self,
        atomic: &nia_function_ir::FunctionAtomic,
        result_ty: nia_ids::InternedTyId,
        span: Span,
    ) {
        // LLVM assigns different legal ordering sets to each atomic opcode and
        // encodes cmpxchg's old value in an aggregate. Validate the complete
        // typed contract here so malformed producer IR cannot reach builder
        // calls whose failure behavior varies across LLVM versions.
        match atomic {
            nia_function_ir::FunctionAtomic::Load { ty, ptr, order } => {
                self.validate_atomic_value_type(*ty, span);
                self.validate_atomic_pointer(ptr.ty, *ty, false, span);
                self.validate_atomic_order(*order, AtomicOrderContext::Load, span);
                self.validate_atomic_result_type(result_ty, *ty, span, "load");
                self.validate_expr(ptr);
            }
            nia_function_ir::FunctionAtomic::Store {
                ty,
                ptr,
                value,
                order,
            } => {
                self.validate_atomic_value_type(*ty, span);
                self.validate_atomic_pointer(ptr.ty, *ty, true, span);
                self.validate_atomic_result_type(value.ty, *ty, span, "store value");
                self.validate_atomic_order(*order, AtomicOrderContext::Store, span);
                self.validate_expr(ptr);
                self.validate_expr(value);
            }
            nia_function_ir::FunctionAtomic::Rmw {
                ty,
                ptr,
                op,
                value,
                order,
            } => {
                self.validate_atomic_value_type(*ty, span);
                self.validate_atomic_pointer(ptr.ty, *ty, true, span);
                self.validate_atomic_result_type(value.ty, *ty, span, "RMW value");
                self.validate_atomic_result_type(result_ty, *ty, span, "RMW result");
                self.validate_atomic_rmw_type(*ty, *op, span);
                self.validate_atomic_order(*order, AtomicOrderContext::Rmw, span);
                self.validate_expr(ptr);
                self.validate_expr(value);
            }
            nia_function_ir::FunctionAtomic::Cmpxchg {
                ty,
                ptr,
                expected,
                desired,
                success,
                failure,
                ..
            } => {
                self.validate_atomic_value_type(*ty, span);
                self.validate_atomic_pointer(ptr.ty, *ty, true, span);
                self.validate_atomic_result_type(expected.ty, *ty, span, "cmpxchg expected value");
                self.validate_atomic_result_type(desired.ty, *ty, span, "cmpxchg desired value");
                match self.index.ty_kind(result_ty).cloned() {
                    Some(TyKind::Optional { elem }) if self.same_type(elem, *ty) => {}
                    _ => self.invalid_atomic_contract(
                        span,
                        "cmpxchg result must be optional over its atomic value type",
                    ),
                }
                self.validate_atomic_order(*success, AtomicOrderContext::CmpxchgSuccess, span);
                self.validate_atomic_order(*failure, AtomicOrderContext::CmpxchgFailure, span);
                if !cmpxchg_failure_order_allowed(*success, *failure) {
                    self.invalid_atomic_contract(
                        span,
                        "cmpxchg failure ordering is stronger than or incomparable with its success ordering",
                    );
                }
                self.validate_expr(ptr);
                self.validate_expr(expected);
                self.validate_expr(desired);
            }
            nia_function_ir::FunctionAtomic::Fence { order } => {
                self.validate_atomic_order(*order, AtomicOrderContext::Fence, span);
            }
        }
    }

    fn validate_atomic_value_type(&mut self, ty: nia_ids::InternedTyId, span: Span) {
        self.validate_runtime_type(ty, span);
        let valid_kind = match self.index.ty_kind(ty) {
            Some(TyKind::Primitive(
                PrimitiveTy::Bool
                | PrimitiveTy::I8
                | PrimitiveTy::I16
                | PrimitiveTy::I32
                | PrimitiveTy::I64
                | PrimitiveTy::Isize
                | PrimitiveTy::U8
                | PrimitiveTy::U16
                | PrimitiveTy::U32
                | PrimitiveTy::U64
                | PrimitiveTy::Usize
                | PrimitiveTy::Char,
            ))
            | Some(TyKind::Pointer { .. }) => true,
            Some(TyKind::Nominal { def_id, .. }) => self.index.has_enum(*def_id),
            _ => false,
        };
        let fits_target = self
            .layout_of(ty)
            .is_some_and(|layout| layout.size <= self.target.pointer_size);
        if !valid_kind || !fits_target {
            self.invalid_atomic_contract(
                span,
                "value type is not a pointer-width bool, integer, enum, or pointer",
            );
        }
    }

    fn validate_atomic_pointer(
        &mut self,
        ptr_ty: nia_ids::InternedTyId,
        value_ty: nia_ids::InternedTyId,
        requires_write: bool,
        span: Span,
    ) {
        match self.index.ty_kind(ptr_ty).cloned() {
            Some(TyKind::Pointer { is_readonly, elem }) => {
                if !self.same_type(elem, value_ty) {
                    self.invalid_atomic_contract(
                        span,
                        "pointer pointee does not match the atomic value type",
                    );
                }
                if requires_write && is_readonly {
                    self.invalid_atomic_contract(span, "mutating operation has a readonly pointer");
                }
            }
            _ => self.invalid_atomic_contract(span, "operand is not a pointer"),
        }
    }

    fn validate_atomic_result_type(
        &mut self,
        actual_ty: nia_ids::InternedTyId,
        expected_ty: nia_ids::InternedTyId,
        span: Span,
        subject: &'static str,
    ) {
        if !self.same_type(actual_ty, expected_ty) {
            self.invalid_atomic_contract(
                span,
                format!("{subject} type does not match the atomic value type"),
            );
        }
    }

    fn validate_atomic_rmw_type(&mut self, ty: nia_ids::InternedTyId, op: AtomicRmwOp, span: Span) {
        if op == AtomicRmwOp::Xchg {
            return;
        }
        let integer_like = match self.index.ty_kind(ty) {
            Some(TyKind::Primitive(
                PrimitiveTy::Bool
                | PrimitiveTy::I8
                | PrimitiveTy::I16
                | PrimitiveTy::I32
                | PrimitiveTy::I64
                | PrimitiveTy::Isize
                | PrimitiveTy::U8
                | PrimitiveTy::U16
                | PrimitiveTy::U32
                | PrimitiveTy::U64
                | PrimitiveTy::Usize
                | PrimitiveTy::Char,
            )) => true,
            Some(TyKind::Nominal { def_id, .. }) => self.index.has_enum(*def_id),
            _ => false,
        };
        if !integer_like {
            self.invalid_atomic_contract(
                span,
                "non-exchange RMW operation requires an integer-like value type",
            );
        }
    }

    fn validate_atomic_order(
        &mut self,
        order: AtomicOrder,
        context: AtomicOrderContext,
        span: Span,
    ) {
        if !context.allows(order) {
            self.invalid_atomic_contract(span, "ordering is invalid for the operation");
        }
    }

    fn invalid_atomic_contract(&mut self, span: Span, message: impl std::fmt::Display) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR atomic operation has an invalid contract: {message}"),
        ));
    }
}

fn cmpxchg_failure_order_allowed(success: AtomicOrder, failure: AtomicOrder) -> bool {
    match success {
        AtomicOrder::Monotonic => failure == AtomicOrder::Monotonic,
        AtomicOrder::Acquire => matches!(failure, AtomicOrder::Monotonic | AtomicOrder::Acquire),
        AtomicOrder::Release => failure == AtomicOrder::Monotonic,
        AtomicOrder::AcqRel => matches!(failure, AtomicOrder::Monotonic | AtomicOrder::Acquire),
        AtomicOrder::SeqCst => matches!(
            failure,
            AtomicOrder::Monotonic | AtomicOrder::Acquire | AtomicOrder::SeqCst
        ),
        AtomicOrder::Unordered => false,
    }
}

impl BackendValidator<'_> {
    fn validate_callee(
        &mut self,
        callee: &FunctionCallee,
        call_args: &[FunctionExpr],
        call_result_ty: nia_ids::InternedTyId,
        span: Span,
    ) {
        match callee {
            FunctionCallee::ClosureEntry { closure_id, state } => {
                self.validate_expr(state);
                let Some(owner) = self.current_closure_owner.clone() else {
                    self.invalid_call_contract(
                        span,
                        "closure-entry",
                        "call has no enclosing closure owner",
                    );
                    return;
                };
                let key = nia_backend_ir::BackendClosureEntryKey {
                    closure_id: *closure_id,
                    owner,
                };
                let Some(entry) = self.index.closure_entry(&key) else {
                    self.invalid_call_contract(
                        span,
                        "closure-entry",
                        "call references a missing generated entry",
                    );
                    return;
                };
                let state_pointer_type = entry.abi.state_pointer_type;
                let params = entry.abi.params.clone();
                let return_type = entry.abi.return_type;
                if !self.same_type(state.ty, state_pointer_type) {
                    self.invalid_call_contract(
                        span,
                        "closure-entry",
                        "state pointer type does not match generated entry ABI",
                    );
                }
                self.validate_typed_call_signature(TypedCallContract {
                    kind: "closure-entry",
                    args: call_args,
                    params: &params,
                    return_type,
                    is_variadic: false,
                    result_ty: call_result_ty,
                    span,
                });
            }
            FunctionCallee::Function(def_id) => {
                self.validate_function_ref(
                    *def_id,
                    span,
                    "backend IR call references missing function",
                );
                if let Some(signature) = self.function_call_signature(*def_id) {
                    self.validate_call_signature(
                        "function",
                        call_args,
                        &signature,
                        call_result_ty,
                        span,
                    );
                }
            }
            FunctionCallee::FunctionInstance {
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
                    span,
                    "backend IR call references missing function instance",
                );
                if let Some(signature) = self.function_instance_call_signature(instance) {
                    self.validate_call_signature(
                        "function-instance",
                        call_args,
                        &signature,
                        call_result_ty,
                        span,
                    );
                }
            }
            FunctionCallee::Method {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
                receiver_kind,
                receiver,
            } => {
                self.validate_expr(receiver);
                let signature = if self_arg.is_none() && args.is_empty() && const_args.is_empty() {
                    self.validate_function_ref(
                        *def_id,
                        span,
                        "backend IR method call references missing function",
                    );
                    self.function_call_signature(*def_id)
                } else {
                    let instance = FunctionInstanceRef {
                        def_id: *def_id,
                        arg_module_id: *arg_module_id,
                        self_arg: *self_arg,
                        args,
                        const_args,
                    };
                    self.validate_function_instance_ref(
                        instance,
                        span,
                        "backend IR method call references missing function instance",
                    );
                    self.function_instance_call_signature(instance)
                };
                if let Some(signature) = signature {
                    self.validate_method_call_signature(
                        call_args,
                        call_result_ty,
                        *receiver_kind,
                        &signature,
                        span,
                    );
                }
            }
            FunctionCallee::DynamicTraitMethod {
                object_ty,
                trait_id,
                method_id,
                trait_args,
                trait_const_args,
                slot,
                params,
                return_type,
                receiver_kind,
                receiver,
                ..
            } => {
                self.validate_type(*object_ty, span);
                self.validate_runtime_type(*return_type, span);
                for param in params {
                    self.validate_runtime_type(*param, span);
                }
                self.validate_expr(receiver);
                self.validate_dynamic_trait_call(DynamicTraitCallContract {
                    object_ty: *object_ty,
                    trait_id: *trait_id,
                    method_id: *method_id,
                    trait_args,
                    trait_const_args,
                    slot: *slot,
                    params,
                    return_type: *return_type,
                    receiver_kind: *receiver_kind,
                    receiver,
                    args: call_args,
                    result_ty: call_result_ty,
                    span,
                });
            }
            FunctionCallee::BuiltinMethod {
                method,
                self_ty,
                receiver,
            } => {
                self.validate_type(*self_ty, span);
                self.validate_expr(receiver);
                if !call_args.is_empty() {
                    self.invalid_call_contract(
                        span,
                        "builtin-method",
                        "builtin methods do not accept value arguments",
                    );
                }
                self.validate_builtin_method_call(*method, *self_ty, call_result_ty, span);
            }
            FunctionCallee::BuiltinPlaceMethod {
                trait_id,
                method,
                self_ty,
                trait_args,
                receiver,
            } => {
                self.validate_type(*self_ty, span);
                for arg in trait_args {
                    self.validate_type(*arg, span);
                }
                self.validate_expr(receiver);
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    span,
                    format!(
                        "backend IR call contains unresolved builtin place method {trait_id:?}::{method:?}"
                    ),
                ));
            }
            FunctionCallee::TraitMethod {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
                receiver,
                ..
            } => {
                self.validate_type(*self_ty, span);
                for arg in trait_args.iter().chain(args) {
                    self.validate_type(*arg, span);
                }
                self.validate_expr(receiver);
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    span,
                    format!(
                        "backend IR call contains unresolved trait method `{}` {method_id:?} on trait {trait_id:?}",
                        mangle_symbol_id(*method_name)
                    ),
                ));
            }
            FunctionCallee::TraitAssociatedFunction {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
                ..
            } => {
                self.validate_type(*self_ty, span);
                for arg in trait_args.iter().chain(args) {
                    self.validate_type(*arg, span);
                }
                self.diagnostics.push(Diagnostic::internal_error_at(nia_diagnostic::codes::INVALID_BACKEND_IR,
                    span,
                    format!(
                        "backend IR call contains unresolved trait associated function `{}` {method_id:?} on trait {trait_id:?}",
                        mangle_symbol_id(*method_name)
                    ),
                ));
            }
            FunctionCallee::Callable(expr) => {
                self.validate_expr(expr);
                let Some(TyKind::Callable {
                    params,
                    return_type,
                    ..
                }) = self.index.ty_kind(expr.ty).cloned()
                else {
                    self.invalid_call_contract(
                        span,
                        "callable",
                        "callee expression does not have callable type",
                    );
                    return;
                };
                self.validate_typed_call_signature(TypedCallContract {
                    kind: "callable",
                    args: call_args,
                    params: &params,
                    return_type,
                    is_variadic: false,
                    result_ty: call_result_ty,
                    span,
                });
            }
            FunctionCallee::FunctionPointer(expr) => {
                self.validate_expr(expr);
                let Some(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                }) = self.index.ty_kind(expr.ty).cloned()
                else {
                    self.invalid_call_contract(
                        span,
                        "function-pointer",
                        "callee expression does not have function-pointer type",
                    );
                    return;
                };
                self.validate_typed_call_signature(TypedCallContract {
                    kind: "function-pointer",
                    args: call_args,
                    params: &params,
                    return_type,
                    is_variadic,
                    result_ty: call_result_ty,
                    span,
                });
            }
            // Intrinsic value operators are intentionally selected in LLVM codegen; backend
            // lowering only rewrites them when a source-level extension method wins dispatch.
            FunctionCallee::BuiltinOperator(operator) => {
                let expected = match operator.op {
                    nia_function_ir::FunctionBuiltinOperatorOp::Unary(_) => 1,
                    nia_function_ir::FunctionBuiltinOperatorOp::Binary(_) => 2,
                };
                if call_args.len() != expected {
                    self.invalid_call_contract(
                        span,
                        "builtin-operator",
                        "argument count does not match operator arity",
                    );
                }
            }
        }
    }

    fn validate_builtin_method_call(
        &mut self,
        method: nia_function_ir::FunctionBuiltinMethod,
        self_ty: nia_ids::InternedTyId,
        result_ty: nia_ids::InternedTyId,
        span: Span,
    ) {
        match method {
            nia_function_ir::FunctionBuiltinMethod::SliceLen => {
                if !matches!(
                    self.index.ty_kind(self_ty),
                    Some(TyKind::Array { .. } | TyKind::Slice { .. })
                ) {
                    self.invalid_call_contract(
                        span,
                        "builtin-method",
                        "len receiver type is not an array or slice",
                    );
                }
                if !matches!(
                    self.index.ty_kind(result_ty),
                    Some(TyKind::Primitive(PrimitiveTy::Usize))
                ) {
                    self.invalid_call_contract(
                        span,
                        "builtin-method",
                        "len result type is not usize",
                    );
                }
            }
            nia_function_ir::FunctionBuiltinMethod::SlicePtr
            | nia_function_ir::FunctionBuiltinMethod::SlicePtrMut => {
                let Some(TyKind::Slice { is_readonly, elem }) = self.index.ty_kind(self_ty) else {
                    self.invalid_call_contract(
                        span,
                        "builtin-method",
                        "pointer receiver type is not a slice",
                    );
                    return;
                };
                let expected_readonly = method == nia_function_ir::FunctionBuiltinMethod::SlicePtr;
                if !expected_readonly && *is_readonly {
                    self.invalid_call_contract(
                        span,
                        "builtin-method",
                        "mutable pointer method has a readonly slice receiver",
                    );
                }
                if !matches!(
                    self.index.ty_kind(result_ty),
                    Some(TyKind::Pointer {
                        is_readonly,
                        elem: result_elem,
                    }) if *is_readonly == expected_readonly
                        && self.same_type(*result_elem, *elem)
                ) {
                    self.invalid_call_contract(
                        span,
                        "builtin-method",
                        "slice pointer result type does not match its receiver",
                    );
                }
            }
            nia_function_ir::FunctionBuiltinMethod::Start
            | nia_function_ir::FunctionBuiltinMethod::End => {
                let Some(TyKind::Range { kind, bound }) = self.index.ty_kind(self_ty) else {
                    self.invalid_call_contract(
                        span,
                        "builtin-method",
                        "range-bound receiver type is not a range",
                    );
                    return;
                };
                let has_bound = match method {
                    nia_function_ir::FunctionBuiltinMethod::Start => kind.has_start_bound(),
                    nia_function_ir::FunctionBuiltinMethod::End => kind.has_end_bound(),
                    _ => unreachable!(),
                };
                if !has_bound {
                    self.invalid_call_contract(
                        span,
                        "builtin-method",
                        "range receiver does not contain the requested bound",
                    );
                }
                if bound.is_none_or(|bound| !self.same_type(bound, result_ty)) {
                    self.invalid_call_contract(
                        span,
                        "builtin-method",
                        "range-bound result type does not match its receiver",
                    );
                }
            }
            nia_function_ir::FunctionBuiltinMethod::Iter => self.invalid_call_contract(
                span,
                "builtin-method",
                "iter must be resolved before LLVM codegen",
            ),
        }
    }

    fn function_call_signature(&self, def_id: nia_ids::GlobalDefId) -> Option<CallTargetSignature> {
        self.index
            .function(def_id)
            .map(|function| CallTargetSignature {
                params: function.params.clone(),
                return_type: function.return_type,
                is_variadic: function.is_variadic,
            })
    }

    fn function_instance_call_signature(
        &self,
        instance: FunctionInstanceRef<'_>,
    ) -> Option<CallTargetSignature> {
        let FunctionInstanceRef {
            def_id,
            arg_module_id,
            self_arg,
            args,
            const_args,
        } = instance;
        self.index
            .function_instance(def_id, arg_module_id, self_arg, args, const_args)
            .or_else(|| {
                self.index.function_instances_for(def_id).find(|item| {
                    self.same_optional_type(item.self_arg, self_arg)
                        && self.same_type_args(&item.args, args)
                        && item.const_args.as_slice() == const_args
                })
            })
            .map(|function| CallTargetSignature {
                params: function.params.clone(),
                return_type: function.return_type,
                is_variadic: function.is_variadic,
            })
    }

    fn validate_function_value_signature(
        &mut self,
        kind: &'static str,
        value_ty: nia_ids::InternedTyId,
        signature: &CallTargetSignature,
        span: Span,
    ) {
        let Some(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        }) = self.index.ty_kind(value_ty)
        else {
            self.invalid_function_value_contract(
                span,
                kind,
                "value type is not a function pointer",
            );
            return;
        };

        // Function-pointer types describe the source-visible signature. In
        // particular, a method receiver may have a pointer-shaped `passing_ty`
        // at the LLVM boundary while retaining its semantic `local_ty` here.
        if params.len() != signature.params.len()
            || params
                .iter()
                .zip(&signature.params)
                .any(|(actual, expected)| !self.same_type(*actual, expected.local_ty))
        {
            self.invalid_function_value_contract(
                span,
                kind,
                "parameter types do not match the published signature",
            );
        }
        if !self.same_type(*return_type, signature.return_type) {
            self.invalid_function_value_contract(
                span,
                kind,
                "return type does not match the published signature",
            );
        }
        if *is_variadic != signature.is_variadic {
            self.invalid_function_value_contract(
                span,
                kind,
                "variadic flag does not match the published signature",
            );
        }
    }

    fn validate_inline_asm(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        asm: &nia_function_ir::FunctionInlineAsm,
        span: Span,
    ) {
        if !matches!(self.ty_kind(result_ty), Some(kind) if kind.is_unit()) {
            self.invalid_inline_asm(span, "expression result type is not unit");
        }
        for input in &asm.inputs {
            self.validate_expr(&input.value);
            if !self.is_inline_asm_operand_type(input.value.ty) {
                self.invalid_inline_asm(input.span, "input operand type is not scalar");
            }
            if !Self::is_inline_asm_input_constraint(&input.constraint) {
                self.invalid_inline_asm(input.span, "input constraint is not canonical");
            }
        }
        for output in &asm.outputs {
            let selected_ty = self.validate_place(&output.place);
            if !self.is_inline_asm_operand_type(output.place.ty) {
                self.invalid_inline_asm(output.span, "output operand type is not scalar");
            }
            if selected_ty.is_some_and(|ty| !self.same_type(output.place.ty, ty)) {
                self.invalid_inline_asm(output.span, "output type is only a readonly storage view");
            }
            if !self.place_is_writable(&output.place) {
                self.invalid_inline_asm(output.span, "output storage is not writable");
            }
            if !Self::is_inline_asm_output_constraint(&output.constraint) {
                self.invalid_inline_asm(output.span, "output constraint is not canonical");
            }
        }
        for clobber in &asm.clobbers {
            if !Self::is_inline_asm_register_name(clobber) {
                self.invalid_inline_asm(span, "clobber name contains constraint syntax");
            }
        }
    }

    fn is_inline_asm_operand_type(&self, ty: nia_ids::InternedTyId) -> bool {
        match self.ty_kind(ty) {
            Some(TyKind::Primitive(primitive)) => *primitive != PrimitiveTy::Never,
            Some(
                TyKind::Pointer { .. }
                | TyKind::VolatilePointer { .. }
                | TyKind::FunctionPointer { .. },
            ) => true,
            // A payload-free enum lowers to its integer tag. Payload enums and
            // all other aggregates would ask LLVM inline assembly to carry a
            // struct value directly, which its constraint interface forbids.
            Some(TyKind::Nominal { def_id, .. }) => {
                self.index.enum_item(*def_id).is_some_and(|item| {
                    item.variants.iter().all(|variant| {
                        matches!(
                            variant.payload,
                            nia_backend_ir::BackendEnumVariantPayload::Unit
                        )
                    })
                })
            }
            _ => false,
        }
    }

    fn is_inline_asm_input_constraint(constraint: &str) -> bool {
        matches!(constraint, "r" | "f")
            || constraint
                .strip_prefix('{')
                .and_then(|constraint| constraint.strip_suffix('}'))
                .is_some_and(Self::is_inline_asm_register_name)
    }

    fn is_inline_asm_output_constraint(constraint: &str) -> bool {
        constraint
            .strip_prefix('=')
            .is_some_and(Self::is_inline_asm_input_constraint)
    }

    fn is_inline_asm_register_name(name: &str) -> bool {
        !name.is_empty()
            && !name.chars().any(|character| {
                character.is_whitespace()
                    || character.is_control()
                    || matches!(character, '{' | '}' | ',')
            })
    }

    fn invalid_inline_asm(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR inline assembly has an invalid contract: {message}"),
        ));
    }

    fn invalid_function_value_contract(
        &mut self,
        span: Span,
        kind: &'static str,
        message: &'static str,
    ) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR {kind} value has an invalid signature contract: {message}"),
        ));
    }

    fn validate_method_call_signature(
        &mut self,
        args: &[FunctionExpr],
        result_ty: nia_ids::InternedTyId,
        receiver_kind: nia_ids::ReceiverKind,
        signature: &CallTargetSignature,
        span: Span,
    ) {
        let Some(target_receiver) = signature.params.first() else {
            self.invalid_call_contract(
                span,
                "method",
                "target signature has no receiver parameter",
            );
            return;
        };
        if target_receiver.receiver != Some(receiver_kind) {
            self.invalid_call_contract(
                span,
                "method",
                "receiver kind does not match target signature",
            );
        }
        let value_signature = CallTargetSignature {
            params: signature.params[1..].to_vec(),
            return_type: signature.return_type,
            is_variadic: signature.is_variadic,
        };
        self.validate_call_signature("method", args, &value_signature, result_ty, span);
    }

    fn validate_call_signature(
        &mut self,
        kind: &'static str,
        args: &[FunctionExpr],
        signature: &CallTargetSignature,
        result_ty: nia_ids::InternedTyId,
        span: Span,
    ) {
        let param_tys = signature
            .params
            .iter()
            .map(|param| param.passing_ty)
            .collect::<Vec<_>>();
        self.validate_typed_call_signature(TypedCallContract {
            kind,
            args,
            params: &param_tys,
            return_type: signature.return_type,
            is_variadic: signature.is_variadic,
            result_ty,
            span,
        });
    }

    fn validate_typed_call_signature(&mut self, call: TypedCallContract<'_>) {
        let TypedCallContract {
            kind,
            args,
            params,
            return_type,
            is_variadic,
            result_ty,
            span,
        } = call;
        if args.len() < params.len() || (!is_variadic && args.len() != params.len()) {
            self.invalid_call_contract(
                span,
                kind,
                "argument count does not match target signature",
            );
        }
        if args
            .iter()
            .zip(params)
            .any(|(arg, param)| !self.call_argument_type_matches(arg.ty, *param))
        {
            self.invalid_call_contract(span, kind, "argument type does not match target signature");
        }
        if !self.same_type(result_ty, return_type) {
            self.invalid_call_contract(span, kind, "result type does not match target signature");
        }
    }

    fn invalid_call_contract(&mut self, span: Span, kind: &'static str, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR {kind} call has an invalid ABI contract: {message}"),
        ));
    }

    fn call_argument_type_matches(
        &self,
        actual: nia_ids::InternedTyId,
        expected: nia_ids::InternedTyId,
    ) -> bool {
        if self.same_type(actual, expected) {
            return true;
        }
        let Some(TyKind::Pointer {
            is_readonly: expected_readonly,
            elem: expected_elem,
        }) = self.index.ty_kind(expected)
        else {
            return false;
        };
        match self.index.ty_kind(actual) {
            Some(TyKind::Pointer {
                is_readonly: actual_readonly,
                elem: actual_elem,
            }) => {
                (*expected_readonly || !*actual_readonly)
                    && self.same_type(*actual_elem, *expected_elem)
            }
            _ => self.same_type(actual, *expected_elem),
        }
    }

    fn validate_dynamic_trait_call(&mut self, call: DynamicTraitCallContract<'_>) {
        let DynamicTraitCallContract {
            object_ty,
            trait_id,
            method_id,
            trait_args,
            trait_const_args,
            slot,
            params,
            return_type,
            receiver_kind,
            receiver,
            args,
            result_ty,
            span,
        } = call;
        if !matches!(
            self.index.ty_kind(object_ty),
            Some(TyKind::TraitObject { .. })
        ) {
            self.invalid_dynamic_trait_call(span, "object type is not a trait object");
        }
        if !self.same_type(receiver.ty, object_ty) {
            self.invalid_dynamic_trait_call(
                span,
                "receiver type does not match its trait-object type",
            );
        }
        for arg in trait_args {
            self.validate_type(*arg, span);
        }
        for arg in trait_const_args {
            self.validate_type(arg.ty, span);
        }
        if !self.same_type(result_ty, return_type) {
            self.invalid_dynamic_trait_call(
                span,
                "expression result type does not match its return metadata",
            );
        }
        if args.len() != params.len() {
            self.invalid_dynamic_trait_call(
                span,
                "argument count does not match its parameter metadata",
            );
        }
        if args
            .iter()
            .zip(params)
            .any(|(arg, param)| !self.call_argument_type_matches(arg.ty, *param))
        {
            self.invalid_dynamic_trait_call(
                span,
                "argument type does not match its parameter metadata",
            );
        }

        let Some(targets) = self.validate_dynamic_trait_slots(
            object_ty,
            VtableTraitInstance {
                trait_id,
                args: trait_args,
                const_args: trait_const_args,
            },
            method_id,
            slot,
            span,
        ) else {
            return;
        };
        for target in targets {
            self.validate_dynamic_trait_target(&target, params, return_type, receiver_kind, span);
        }
    }

    fn validate_dynamic_trait_target(
        &mut self,
        target: &nia_backend_ir::BackendTraitObjectVtableFunction,
        params: &[nia_ids::InternedTyId],
        return_type: nia_ids::InternedTyId,
        receiver_kind: nia_ids::ReceiverKind,
        span: Span,
    ) {
        let Some((target_params, target_return_type)) = self.dynamic_trait_target_signature(target)
        else {
            self.invalid_dynamic_trait_call(span, "vtable slot references a missing function");
            return;
        };
        let Some(target_receiver) = target_params.first() else {
            self.invalid_dynamic_trait_call(span, "vtable target has no receiver parameter");
            return;
        };
        if target_receiver.receiver != Some(receiver_kind) {
            self.invalid_dynamic_trait_call(
                span,
                "receiver kind does not match the vtable target signature",
            );
        }
        let target_value_params = &target_params[1..];
        if target_value_params.len() != params.len()
            || target_value_params
                .iter()
                .zip(params)
                .any(|(target, param)| !self.same_type(target.passing_ty, *param))
        {
            self.invalid_dynamic_trait_call(
                span,
                "parameter metadata does not match the vtable target signature",
            );
        }
        if !self.same_type(target_return_type, return_type) {
            self.invalid_dynamic_trait_call(
                span,
                "return metadata does not match the vtable target signature",
            );
        }
    }

    fn validate_dynamic_trait_slots(
        &mut self,
        object_ty: nia_ids::InternedTyId,
        trait_instance: VtableTraitInstance<'_>,
        method_id: nia_ids::GlobalDefId,
        slot: usize,
        span: Span,
    ) -> Option<Vec<nia_backend_ir::BackendTraitObjectVtableFunction>> {
        // The slot is part of the typed call contract, not merely an indexing
        // hint. Calls on the original object use absolute slots in its complete
        // table; explicitly upcast views use slots relative to the target
        // supertrait segment. Keep those two representations distinct so a
        // malformed slot cannot turn into an unchecked LLVM GEP. Every
        // concrete table is checked: selecting the first table would make ABI
        // validation depend on module publication order.
        let exact_vtables = self
            .index
            .trait_object_vtables_for_object_ty(object_ty)
            .filter(|vtable| self.same_type(vtable.key.object_ty, object_ty))
            .chain(
                self.index
                    .trait_object_vtables_for_trait(trait_instance.trait_id)
                    .filter(|vtable| self.same_type(vtable.key.object_ty, object_ty)),
            )
            .collect::<Vec<_>>();
        let mut targets = Vec::new();
        for vtable in &exact_vtables {
            let Some(entry) =
                self.dynamic_trait_slot_entry(vtable, trait_instance, method_id, slot)
            else {
                self.invalid_dynamic_trait_slot(span);
                return None;
            };
            if !targets.contains(&entry.function) {
                targets.push(entry.function.clone());
            }
        }

        // An explicitly upcast receiver names the target trait-object type but
        // retains a pointer into a source vtable. A direct table and one or
        // more such source tables can coexist, so both sets are runtime
        // candidates. The source-table offset is anchored at the object view's
        // principal trait segment, not the trait that happened to declare the
        // called method; otherwise calls to that principal trait's supertraits
        // would be validated against the wrong relative slot.
        let object_trait_instance = self.vtable_trait_instance_for_object_ty(object_ty)?;
        for vtable in self
            .index
            .trait_object_vtables()
            .filter(|vtable| !self.same_type(vtable.key.object_ty, object_ty))
        {
            let Some(first_slot) =
                self.first_vtable_slot_for_trait_instance(vtable, object_trait_instance)
            else {
                continue;
            };
            let Some(entry) = self.dynamic_trait_upcast_slot_entry(
                vtable,
                trait_instance,
                method_id,
                first_slot,
                slot,
            ) else {
                self.invalid_dynamic_trait_slot(span);
                return None;
            };
            if !targets.contains(&entry.function) {
                targets.push(entry.function.clone());
            }
        }
        // A dynamic call can consume a trait object supplied at runtime even
        // when this closed program never constructs a concrete object of that
        // type. Validate every materialized runtime candidate, but do not
        // require one merely to validate the call contract itself.
        Some(targets)
    }

    fn invalid_dynamic_trait_slot(&mut self, span: Span) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            "backend IR dynamic trait call has an invalid vtable method slot",
        ));
    }

    fn dynamic_trait_slot_entry<'a>(
        &self,
        vtable: &'a nia_backend_ir::BackendTraitObjectVtable,
        trait_instance: VtableTraitInstance<'_>,
        method_id: nia_ids::GlobalDefId,
        slot: usize,
    ) -> Option<&'a nia_backend_ir::BackendTraitObjectVtableEntry> {
        vtable.entries.iter().find(|entry| {
            self.vtable_entry_matches_trait_instance(entry, trait_instance)
                && entry.method_id == method_id
                && entry.slot == slot
        })
    }

    fn dynamic_trait_upcast_slot_entry<'a>(
        &self,
        vtable: &'a nia_backend_ir::BackendTraitObjectVtable,
        trait_instance: VtableTraitInstance<'_>,
        method_id: nia_ids::GlobalDefId,
        first_slot: usize,
        slot: usize,
    ) -> Option<&'a nia_backend_ir::BackendTraitObjectVtableEntry> {
        vtable.entries.iter().find(|entry| {
            self.vtable_entry_matches_trait_instance(entry, trait_instance)
                && entry.method_id == method_id
                && entry.slot.checked_sub(first_slot) == Some(slot)
        })
    }

    fn vtable_trait_instance_for_object_ty(
        &self,
        object_ty: nia_ids::InternedTyId,
    ) -> Option<VtableTraitInstance<'_>> {
        let TyKind::TraitObject {
            trait_id,
            trait_args,
            trait_const_args,
            ..
        } = self.index.ty_kind(object_ty)?
        else {
            return None;
        };
        Some(VtableTraitInstance {
            trait_id: *trait_id,
            args: trait_args,
            const_args: trait_const_args,
        })
    }

    fn first_vtable_slot_for_trait_instance(
        &self,
        vtable: &nia_backend_ir::BackendTraitObjectVtable,
        trait_instance: VtableTraitInstance<'_>,
    ) -> Option<usize> {
        vtable
            .entries
            .iter()
            .filter(|entry| self.vtable_entry_matches_trait_instance(entry, trait_instance))
            .map(|entry| entry.slot)
            .min()
    }

    fn vtable_entry_matches_trait_instance(
        &self,
        entry: &nia_backend_ir::BackendTraitObjectVtableEntry,
        trait_instance: VtableTraitInstance<'_>,
    ) -> bool {
        entry.trait_id == trait_instance.trait_id
            && self.same_type_args(&entry.trait_args, trait_instance.args)
            && entry.trait_const_args.len() == trait_instance.const_args.len()
            && entry
                .trait_const_args
                .iter()
                .zip(trait_instance.const_args)
                .all(|(entry_arg, call_arg)| {
                    entry_arg.value == call_arg.value && self.same_type(entry_arg.ty, call_arg.ty)
                })
    }

    fn dynamic_trait_target_signature(
        &self,
        target: &nia_backend_ir::BackendTraitObjectVtableFunction,
    ) -> Option<(Vec<nia_backend_ir::BackendParam>, nia_ids::InternedTyId)> {
        match target {
            nia_backend_ir::BackendTraitObjectVtableFunction::Function(def_id) => self
                .index
                .function(*def_id)
                .map(|function| (function.params.clone(), function.return_type)),
            nia_backend_ir::BackendTraitObjectVtableFunction::FunctionInstance {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
            } => self
                .index
                .function_instance(*def_id, *arg_module_id, *self_arg, args, const_args)
                .map(|function| (function.params.clone(), function.return_type)),
        }
    }

    fn invalid_dynamic_trait_call(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR dynamic trait call has an invalid ABI contract: {message}"),
        ));
    }

    fn validate_place(&mut self, place: &FunctionPlace) -> Option<nia_ids::InternedTyId> {
        self.current_subject = Some("place");
        self.validate_runtime_type(place.ty, place.span);
        self.current_subject = None;
        let valid_base = match &place.base {
            FunctionPlaceBase::Local(local_id) => {
                let exists = self
                    .local_tys
                    .last()
                    .is_some_and(|local_tys| local_tys.contains_key(local_id));
                if !exists {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        place.span,
                        format!("backend IR place references missing local {local_id:?}"),
                    ));
                }
                exists
            }
            FunctionPlaceBase::Global(def_id) => {
                let exists = self.index.has_global(*def_id);
                if !exists {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        place.span,
                        format!("backend IR place references missing global {def_id:?}"),
                    ));
                }
                exists
            }
            FunctionPlaceBase::GlobalInstance {
                def_id,
                arg_module_id,
                args,
                const_args,
            } => {
                let exists = self
                    .index
                    .global_instance(*def_id, *arg_module_id, args, const_args)
                    .is_some();
                if !exists {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        place.span,
                        format!("backend IR place references missing global instance {def_id:?}"),
                    ));
                }
                exists
            }
            FunctionPlaceBase::Deref(expr) => {
                self.validate_expr(expr);
                if matches!(
                    self.ty_kind(expr.ty),
                    Some(TyKind::Pointer { .. } | TyKind::VolatilePointer { .. })
                ) {
                    true
                } else {
                    self.invalid_place(place.span, "deref base is not a pointer");
                    false
                }
            }
            FunctionPlaceBase::Error => false,
        };
        if !valid_base {
            return None;
        }
        let selected_ty = self.validate_place_path(place)?;
        let place_storage_ty = match self.ty_kind(place.ty) {
            Some(TyKind::Pointer { elem, .. } | TyKind::VolatilePointer { elem, .. })
                if self.same_type(*elem, selected_ty) =>
            {
                *elem
            }
            _ => place.ty,
        };
        if !self.projection_result_compatible(place_storage_ty, selected_ty) {
            self.invalid_place(
                place.span,
                "result type does not match the selected storage",
            );
        }
        Some(selected_ty)
    }

    fn validate_assignment(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        place: &FunctionPlace,
        op: AssignOp,
        rhs: &FunctionExpr,
        span: Span,
    ) {
        let selected_ty = self.validate_place(place);
        self.validate_expr(rhs);
        if !matches!(self.ty_kind(result_ty), Some(TyKind::Tuple(elems)) if elems.is_empty()) {
            self.invalid_assignment(span, "result type is not unit");
        }
        let Some(selected_ty) = selected_ty else {
            return;
        };
        // Read expressions may expose a readonly view of mutable storage, but a
        // store must retain the storage's exact type or LLVM can accept a write
        // whose source-level pointee qualifiers no longer match.
        if !self.same_type(place.ty, selected_ty) {
            self.invalid_assignment(span, "target type is only a readonly storage view");
        }
        if !self.place_is_writable(place) {
            self.invalid_assignment(span, "target storage is not writable");
        }
        if op == AssignOp::Assign {
            if !self.same_type(place.ty, rhs.ty) {
                self.invalid_assignment(span, "right-hand side type does not match the target");
            }
        } else if let Some(binary_op) = assign_to_binary_op(op) {
            self.validate_binary_contract(place.ty, place.ty, binary_op, rhs.ty, span);
        }
    }

    fn place_is_writable(&self, place: &FunctionPlace) -> bool {
        let mut writable = match &place.base {
            FunctionPlaceBase::Local(local_id) => self
                .local_kinds
                .last()
                .and_then(|locals| locals.get(local_id))
                .is_some_and(|kind| *kind != nia_function_ir::FunctionLocalKind::ImmutableBinding),
            FunctionPlaceBase::Global(def_id) => self
                .index
                .global(*def_id)
                .is_some_and(|global| !global.is_let),
            FunctionPlaceBase::GlobalInstance {
                def_id,
                arg_module_id,
                args,
                const_args,
            } => self
                .index
                .global_instance(*def_id, *arg_module_id, args, const_args)
                .is_some_and(|global| !global.is_let),
            FunctionPlaceBase::Deref(expr) => matches!(
                self.ty_kind(expr.ty),
                Some(
                    TyKind::Pointer {
                        is_readonly: false,
                        ..
                    } | TyKind::VolatilePointer {
                        is_readonly: false,
                        ..
                    }
                )
            ),
            FunctionPlaceBase::Error => false,
        };
        let Some(mut current_ty) = self.place_base_ty(place) else {
            return false;
        };
        for elem in &place.elems {
            match elem {
                FunctionPlaceElem::Field(field) => {
                    if let Some(
                        TyKind::Pointer { elem, .. } | TyKind::VolatilePointer { elem, .. },
                    ) = self.ty_kind(current_ty)
                    {
                        current_ty = *elem;
                    }
                    let Some((def_id, args, const_args)) = self.field_base_type(current_ty) else {
                        return false;
                    };
                    let Some(field_ty) = self
                        .aggregate_fields(def_id, &args, &const_args)
                        .and_then(|fields| {
                            fields.iter().find(|candidate| candidate.def_id == *field)
                        })
                        .map(|field| field.ty)
                    else {
                        return false;
                    };
                    current_ty = field_ty;
                }
                FunctionPlaceElem::TupleField(index) => {
                    let Some(
                        TyKind::Tuple(elems)
                        | TyKind::ClosureState {
                            captures: elems, ..
                        },
                    ) = self.ty_kind(current_ty)
                    else {
                        return false;
                    };
                    let Some(elem) = elems.get(*index) else {
                        return false;
                    };
                    current_ty = *elem;
                }
                FunctionPlaceElem::Index(_) => {
                    let Some(elem_ty) = self.array_elem_ty(current_ty) else {
                        return false;
                    };
                    if matches!(
                        self.ty_kind(current_ty),
                        Some(
                            TyKind::Pointer {
                                is_readonly: true,
                                ..
                            } | TyKind::VolatilePointer {
                                is_readonly: true,
                                ..
                            } | TyKind::Slice {
                                is_readonly: true,
                                ..
                            }
                        )
                    ) {
                        writable = false;
                    }
                    current_ty = elem_ty;
                }
                FunctionPlaceElem::Error => return false,
            }
        }
        writable
    }

    fn invalid_assignment(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR assignment has an invalid contract: {message}"),
        ));
    }

    fn invalid_projection(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR projection has an invalid contract: {message}"),
        ));
    }

    fn validate_addr_of_result(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        place: &FunctionPlace,
        span: Span,
    ) {
        let Some(TyKind::Pointer { elem, .. }) = self.ty_kind(result_ty) else {
            self.invalid_place(span, "address-of result is not a pointer");
            return;
        };
        let place_storage_ty = match self.ty_kind(place.ty) {
            Some(
                TyKind::Pointer {
                    elem: place_elem, ..
                }
                | TyKind::VolatilePointer {
                    elem: place_elem, ..
                },
            ) if self.same_type(*place_elem, *elem) => *place_elem,
            _ => place.ty,
        };
        if !self.same_type(*elem, place_storage_ty) {
            self.invalid_place(span, "address-of pointee does not match its place type");
        }
    }

    fn invalid_place(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR place has an invalid type contract: {message}"),
        ));
    }

    fn validate_place_path(&mut self, place: &FunctionPlace) -> Option<nia_ids::InternedTyId> {
        let mut current_ty = self.place_base_ty(place)?;
        for elem in &place.elems {
            match elem {
                FunctionPlaceElem::Field(field) => {
                    if let Some(
                        TyKind::Pointer { elem, .. } | TyKind::VolatilePointer { elem, .. },
                    ) = self.ty_kind(current_ty)
                    {
                        current_ty = *elem;
                    }
                    current_ty = self.validate_aggregate_field(
                        current_ty,
                        *field,
                        place.span,
                        "backend IR place references missing field",
                    )?;
                }
                FunctionPlaceElem::TupleField(index) => {
                    let Some(
                        TyKind::Tuple(elems)
                        | TyKind::ClosureState {
                            captures: elems, ..
                        },
                    ) = self.ty_kind(current_ty)
                    else {
                        self.invalid_place(place.span, "tuple projection target is not a tuple");
                        return None;
                    };
                    let Some(elem) = elems.get(*index) else {
                        self.invalid_place(place.span, "tuple projection is out of bounds");
                        return None;
                    };
                    current_ty = *elem;
                }
                FunctionPlaceElem::Index(expr) => {
                    self.validate_expr(expr);
                    if !self.is_integer_type(expr.ty) {
                        self.invalid_place(expr.span, "index is not an integer");
                    }
                    let Some(elem_ty) = self.array_elem_ty(current_ty) else {
                        self.invalid_place(place.span, "index target is not indexable storage");
                        return None;
                    };
                    current_ty = elem_ty;
                }
                FunctionPlaceElem::Error => return None,
            }
        }
        Some(current_ty)
    }

    fn const_generic_value_name(&self, value: &ConstGenericValue) -> String {
        match value {
            ConstGenericValue::GenericParam(name) => mangle_symbol_id(*name),
            ConstGenericValue::ConstExpr(id) => format!("{id:?}"),
            ConstGenericValue::Int(value) => value.bits().to_string(),
            ConstGenericValue::Bool(value) => value.to_string(),
            ConstGenericValue::Char(value) => value.to_string(),
        }
    }
}
