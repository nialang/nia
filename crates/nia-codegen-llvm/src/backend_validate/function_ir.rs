// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashSet;

use nia_ast::AssignOp;
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
mod builtin_values;
mod callables;
mod calls;
mod control_flow;
mod enum_values;
mod literals;
mod locals;
mod low_level;
mod memory;
mod operators;
mod places;
mod projections;
mod range_values;
mod tagged_union;
mod trait_objects;

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
}
