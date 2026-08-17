// SPDX-License-Identifier: GPL-3.0-or-later
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    FunctionArrayElements, FunctionBody, FunctionCallee, FunctionDeferBody, FunctionExpr,
    FunctionExprKind, FunctionOp, FunctionPlace, FunctionPlaceBase, FunctionPlaceElem,
    FunctionTerminator, FunctionTryKind, validate_function_body,
};
use nia_mangle::mangle_symbol_id;
use nia_span::Span;
use nia_ty::{ConstGenericValue, PrimitiveTy, TyKind};

use super::{BackendValidator, FunctionInstanceRef};

struct DynamicTraitCallContract<'a> {
    object_ty: nia_ids::InternedTyId,
    trait_id: nia_ty::TraitId,
    method_id: nia_ids::GlobalDefId,
    slot: usize,
    params: &'a [nia_ids::InternedTyId],
    return_type: nia_ids::InternedTyId,
    receiver_kind: nia_ids::ReceiverKind,
    receiver: &'a FunctionExpr,
    args: &'a [FunctionExpr],
    result_ty: nia_ids::InternedTyId,
    span: Span,
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
            FunctionOp::MemoryIntrinsic(memory) => {
                self.validate_expr(&memory.dest);
                match &memory.source {
                    nia_function_ir::FunctionMemoryIntrinsicSource::Slice(source)
                    | nia_function_ir::FunctionMemoryIntrinsicSource::Byte(source) => {
                        self.validate_expr(source);
                    }
                }
            }
            FunctionOp::Defer(body) => self.validate_defer_body(body),
        }
    }

    fn validate_terminator(&mut self, terminator: &FunctionTerminator) {
        match terminator {
            FunctionTerminator::If { cond, span, .. } => {
                self.validate_expr(cond);
                self.validate_bool_condition(cond.ty, *span);
            }
            FunctionTerminator::Switch { target, arms, .. } => {
                self.validate_expr(target);
                for arm in arms {
                    self.validate_expr(&arm.pattern);
                    if !self.same_type(target.ty, arm.pattern.ty) {
                        self.invalid_terminator(
                            arm.pattern.span,
                            "switch arm pattern type does not match its target",
                        );
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
                if !self.index.has_global(*def_id) {
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
                if self
                    .index
                    .global_instance(*def_id, *arg_module_id, args, const_args)
                    .is_none()
                {
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
            }
            FunctionExprKind::FunctionInstance {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
            } => {
                self.validate_function_instance_ref(
                    FunctionInstanceRef {
                        def_id: *def_id,
                        arg_module_id: *arg_module_id,
                        self_arg: *self_arg,
                        args,
                        const_args,
                    },
                    expr.span,
                    "backend IR expression references missing function instance",
                );
            }
            FunctionExprKind::Range(range) => {
                if let Some(start) = &range.start {
                    self.validate_expr(start);
                }
                if let Some(end) = &range.end {
                    self.validate_expr(end);
                }
            }
            FunctionExprKind::RangeBound { range, .. } => self.validate_expr(range),
            FunctionExprKind::InlineAsm(asm) => {
                for input in &asm.inputs {
                    self.validate_expr(&input.value);
                }
                for output in &asm.outputs {
                    self.validate_place(&output.place);
                }
            }
            FunctionExprKind::Atomic(atomic) => self.validate_atomic(atomic),
            FunctionExprKind::StaticArrayPointer {
                allocation, array, ..
            } => {
                if self.index.module(allocation.module_id()).is_none() {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        expr.span,
                        "backend IR static array pointer references a missing origin module",
                    ));
                }
                self.validate_expr(array);
            }
            FunctionExprKind::ArrayLiteral { elems } => match elems {
                FunctionArrayElements::List(elems) => {
                    for elem in elems {
                        self.validate_expr(elem);
                    }
                }
                FunctionArrayElements::Repeat { value, .. } => self.validate_expr(value),
            },
            FunctionExprKind::Tuple(elems) => {
                for elem in elems {
                    self.validate_expr(elem);
                }
            }
            FunctionExprKind::TupleField { value, index } => {
                self.validate_expr(value);
                match self.index.ty_kind(value.ty) {
                    Some(
                        TyKind::Tuple(elems)
                        | TyKind::ClosureState {
                            captures: elems, ..
                        },
                    ) if *index < elems.len() => {}
                    Some(TyKind::Tuple(_) | TyKind::ClosureState { .. }) => {
                        self.diagnostics.push(Diagnostic::internal_error_at(
                            nia_diagnostic::codes::INVALID_BACKEND_IR,
                            expr.span,
                            "backend IR tuple projection is out of bounds",
                        ))
                    }
                    _ => self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        expr.span,
                        "backend IR tuple projection target is not a tuple",
                    )),
                }
            }
            FunctionExprKind::StructLiteral { def_id, fields } => {
                self.validate_aggregate_def(
                    *def_id,
                    expr.span,
                    "backend IR struct literal references missing struct",
                );
                for field in fields {
                    self.validate_field_init(expr.ty, field.field, field.span);
                    self.validate_expr(&field.value);
                }
            }
            FunctionExprKind::UnionLiteral { def_id, field } => {
                self.validate_aggregate_def(
                    *def_id,
                    expr.span,
                    "backend IR union literal references missing union",
                );
                self.validate_field_init(expr.ty, field.field, field.span);
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
            | FunctionExprKind::TraitObjectCoercion { expr, .. } => self.validate_expr(expr),
            FunctionExprKind::CallableCoercion { state, .. } => {
                self.validate_expr(state);
            }
            FunctionExprKind::ClosureFunctionPointer { .. } => match self.index.ty_kind(expr.ty) {
                Some(TyKind::FunctionPointer {
                    is_variadic: false, ..
                }) => {}
                _ => self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    expr.span,
                    "closure function pointer expression has a non-function-pointer type",
                )),
            },
            FunctionExprKind::AddrOf(place) => self.validate_place(place),
            FunctionExprKind::Binary { lhs, rhs, .. } => {
                self.validate_expr(lhs);
                self.validate_expr(rhs);
            }
            FunctionExprKind::ExtractElement { vector, index } => {
                self.validate_expr(vector);
                self.validate_expr(index);
            }
            FunctionExprKind::InsertElement {
                vector,
                index,
                value,
            } => {
                self.validate_expr(vector);
                self.validate_expr(index);
                self.validate_expr(value);
            }
            FunctionExprKind::Assign { place, rhs, .. } => {
                self.validate_place(place);
                self.validate_expr(rhs);
            }
            FunctionExprKind::Call { callee, args } => {
                self.validate_callee(callee, args, expr.ty, expr.span);
                for arg in args {
                    self.validate_expr(arg);
                }
            }
            FunctionExprKind::Field { lhs, field } => {
                self.validate_expr(lhs);
                self.validate_aggregate_field(
                    lhs.ty,
                    *field,
                    expr.span,
                    "backend IR field expression references missing field",
                );
            }
            FunctionExprKind::Index { lhs, index } => {
                self.validate_expr(lhs);
                self.validate_expr(index);
            }
            FunctionExprKind::Slice { lhs, range, .. } => {
                self.validate_expr(lhs);
                if let Some(start) = &range.start {
                    self.validate_expr(start);
                }
                if let Some(end) = &range.end {
                    self.validate_expr(end);
                }
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
            | FunctionExprKind::BuiltinValue(_)
            | FunctionExprKind::Trap => {}
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
                self.validate_enum_variant_ref(
                    *variant,
                    expr.span,
                    "backend IR expression references missing enum variant",
                );
                for field in fields {
                    self.validate_expr(field);
                }
            }
            FunctionExprKind::EnumVariantTag(variant) => {
                self.validate_enum_variant_ref(
                    *variant,
                    expr.span,
                    "backend IR expression references missing enum variant tag",
                );
            }
            FunctionExprKind::EnumTag { value } => self.validate_expr(value),
            FunctionExprKind::EnumPayloadField { value, variant, .. } => {
                self.validate_expr(value);
                self.validate_enum_variant_ref(
                    *variant,
                    expr.span,
                    "backend IR expression references missing enum payload variant",
                );
            }
        }
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

    fn validate_atomic(&mut self, atomic: &nia_function_ir::FunctionAtomic) {
        match atomic {
            nia_function_ir::FunctionAtomic::Load { ptr, .. } => {
                self.validate_expr(ptr);
            }
            nia_function_ir::FunctionAtomic::Store { ptr, value, .. }
            | nia_function_ir::FunctionAtomic::Rmw { ptr, value, .. } => {
                self.validate_expr(ptr);
                self.validate_expr(value);
            }
            nia_function_ir::FunctionAtomic::Cmpxchg {
                ptr,
                expected,
                desired,
                ..
            } => {
                self.validate_expr(ptr);
                self.validate_expr(expected);
                self.validate_expr(desired);
            }
            nia_function_ir::FunctionAtomic::Fence { .. } => {}
        }
    }

    fn validate_callee(
        &mut self,
        callee: &FunctionCallee,
        call_args: &[FunctionExpr],
        call_result_ty: nia_ids::InternedTyId,
        span: Span,
    ) {
        match callee {
            FunctionCallee::ClosureEntry { state, .. } => {
                self.validate_expr(state);
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
                self_ty, receiver, ..
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

        let Some(target) =
            self.validate_dynamic_trait_slot(object_ty, trait_id, method_id, slot, span)
        else {
            return;
        };
        let Some((target_params, target_return_type)) =
            self.dynamic_trait_target_signature(&target)
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

    fn validate_dynamic_trait_slot(
        &mut self,
        object_ty: nia_ids::InternedTyId,
        trait_id: nia_ty::TraitId,
        method_id: nia_ids::GlobalDefId,
        slot: usize,
        span: Span,
    ) -> Option<nia_backend_ir::BackendTraitObjectVtableFunction> {
        // The slot is part of the typed call contract, not merely an indexing
        // hint. Resolve it against the exact object vtable first, then the
        // trait index for equivalent handles, so a malformed slot cannot turn
        // into an unchecked LLVM GEP.
        let vtable = self
            .index
            .trait_object_vtables_for_object_ty(object_ty)
            .find(|vtable| self.same_type(vtable.key.object_ty, object_ty))
            .or_else(|| {
                self.index
                    .trait_object_vtables_for_trait(trait_id)
                    .find(|vtable| self.same_type(vtable.key.object_ty, object_ty))
            })
            .or_else(|| {
                // An upcast receiver keeps the source vtable metadata while
                // its typed view names a supertrait object. Such a vtable is
                // not indexed under the target object type, so validate the
                // method identity against the emitted source tables as well.
                self.index.trait_object_vtables().find(|vtable| {
                    Self::dynamic_trait_slot_entry(vtable, trait_id, method_id, slot).is_some()
                })
            });
        let Some(vtable) = vtable else {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                "backend IR dynamic trait call has no matching object vtable",
            ));
            return None;
        };
        let Some(entry) = Self::dynamic_trait_slot_entry(vtable, trait_id, method_id, slot) else {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                "backend IR dynamic trait call has an invalid vtable method slot",
            ));
            return None;
        };
        Some(entry.function.clone())
    }

    fn dynamic_trait_slot_entry(
        vtable: &nia_backend_ir::BackendTraitObjectVtable,
        trait_id: nia_ids::TraitId,
        method_id: nia_ids::GlobalDefId,
        slot: usize,
    ) -> Option<&nia_backend_ir::BackendTraitObjectVtableEntry> {
        let first_slot = vtable
            .entries
            .iter()
            .filter(|entry| entry.trait_id == trait_id)
            .map(|entry| entry.slot)
            .min()?;
        vtable.entries.iter().find(|entry| {
            entry.trait_id == trait_id
                && entry.method_id == method_id
                && entry.slot.checked_sub(first_slot) == Some(slot)
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

    fn validate_place(&mut self, place: &FunctionPlace) {
        self.current_subject = Some("place");
        self.validate_runtime_type(place.ty, place.span);
        self.current_subject = None;
        match &place.base {
            FunctionPlaceBase::Local(local_id) => {
                if !self
                    .local_tys
                    .last()
                    .is_some_and(|local_tys| local_tys.contains_key(local_id))
                {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        place.span,
                        format!("backend IR place references missing local {local_id:?}"),
                    ));
                }
            }
            FunctionPlaceBase::Global(def_id) => {
                if !self.index.has_global(*def_id) {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        place.span,
                        format!("backend IR place references missing global {def_id:?}"),
                    ));
                }
            }
            FunctionPlaceBase::GlobalInstance {
                def_id,
                arg_module_id,
                args,
                const_args,
            } => {
                if self
                    .index
                    .global_instance(*def_id, *arg_module_id, args, const_args)
                    .is_none()
                {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        place.span,
                        format!("backend IR place references missing global instance {def_id:?}"),
                    ));
                }
            }
            FunctionPlaceBase::Deref(expr) => self.validate_expr(expr),
            FunctionPlaceBase::Error => {}
        }
        for elem in &place.elems {
            match elem {
                FunctionPlaceElem::Index(expr) => self.validate_expr(expr),
                FunctionPlaceElem::Field(_) | FunctionPlaceElem::TupleField(_) => {
                    if self.place_base_ty(place).is_some() {
                        self.validate_place_path(place);
                    }
                    break;
                }
                FunctionPlaceElem::Error => {}
            }
        }
    }

    fn validate_place_path(&mut self, place: &FunctionPlace) {
        let Some(mut current_ty) = self.place_base_ty(place) else {
            return;
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
                    if let Some(field_ty) = self.validate_aggregate_field(
                        current_ty,
                        *field,
                        place.span,
                        "backend IR place references missing field",
                    ) {
                        current_ty = field_ty;
                    }
                }
                FunctionPlaceElem::TupleField(index) => {
                    let Some(
                        TyKind::Tuple(elems)
                        | TyKind::ClosureState {
                            captures: elems, ..
                        },
                    ) = self.ty_kind(current_ty)
                    else {
                        self.diagnostics.push(Diagnostic::internal_error_at(
                            nia_diagnostic::codes::INVALID_BACKEND_IR,
                            place.span,
                            "backend IR tuple place projection target is not a tuple",
                        ));
                        continue;
                    };
                    if let Some(elem) = elems.get(*index) {
                        current_ty = *elem;
                    } else {
                        self.diagnostics.push(Diagnostic::internal_error_at(
                            nia_diagnostic::codes::INVALID_BACKEND_IR,
                            place.span,
                            "backend IR tuple place projection is out of bounds",
                        ));
                    }
                }
                FunctionPlaceElem::Index(expr) => {
                    self.validate_expr(expr);
                    if let Some(elem_ty) = self.array_elem_ty(current_ty) {
                        current_ty = elem_ty;
                    }
                }
                FunctionPlaceElem::Error => {}
            }
        }
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
