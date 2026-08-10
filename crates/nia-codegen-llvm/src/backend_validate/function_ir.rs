// SPDX-License-Identifier: GPL-3.0-or-later
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    FunctionArrayElements, FunctionBody, FunctionCallee, FunctionDeferBody, FunctionExpr,
    FunctionExprKind, FunctionOp, FunctionPlace, FunctionPlaceBase, FunctionPlaceElem,
    FunctionTerminator, validate_function_body,
};
use nia_mangle::mangle_symbol_id;
use nia_span::Span;
use nia_ty::{ConstGenericValue, TyKind};

use super::{BackendValidator, FunctionInstanceRef};

impl BackendValidator<'_> {
    pub(super) fn validate_function_body(&mut self, body: &FunctionBody) {
        if let Err(error) = validate_function_body(body) {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                error.span,
                format!("backend IR contains invalid function IR: {}", error.message),
            ));
            return;
        }
        self.validate_type(body.ty, body.span);
        self.local_tys.push(
            body.locals
                .iter()
                .map(|local| (local.id, local.ty))
                .collect(),
        );
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
        }
        self.local_tys.pop();
    }

    fn validate_defer_body(&mut self, body: &FunctionDeferBody) {
        for block in &body.blocks {
            for op in &block.ops {
                self.validate_op(op);
            }
            self.validate_terminator(&block.terminator);
        }
    }

    fn validate_op(&mut self, op: &FunctionOp) {
        match op {
            FunctionOp::Binding(binding) => {
                self.current_subject = Some("binding");
                self.validate_runtime_type(binding.ty, Span::default());
                self.current_subject = None;
                if let Some(local_tys) = self.local_tys.last_mut() {
                    local_tys.insert(binding.local_id, binding.ty);
                }
                if let Some(value) = &binding.value {
                    self.validate_expr(value);
                }
            }
            FunctionOp::StoreLocal { value, .. } | FunctionOp::Expr(value) => {
                self.validate_expr(value);
            }
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
            FunctionTerminator::If { cond, .. } => self.validate_expr(cond),
            FunctionTerminator::Switch { target, arms, .. } => {
                self.validate_expr(target);
                for arm in arms {
                    self.validate_expr(&arm.pattern);
                }
            }
            FunctionTerminator::Try {
                value,
                error_conversion,
                ..
            } => {
                self.validate_expr(value);
                if let Some(conversion) = error_conversion {
                    self.validate_expr(conversion);
                }
            }
            FunctionTerminator::Loop { header, .. } => match header {
                nia_function_ir::FunctionForHeader::Infinite => {}
                nia_function_ir::FunctionForHeader::Condition(expr) => self.validate_expr(expr),
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
        self.validate_type(expr.ty, expr.span);
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
                    Some(TyKind::Tuple(elems)) if *index < elems.len() => {}
                    Some(TyKind::Tuple(_)) => self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        expr.span,
                        "backend IR tuple projection is out of bounds",
                    )),
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
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    expr.span,
                    "callable view construction reached LLVM before closure entry materialization",
                ));
            }
            FunctionExprKind::ClosureFunctionPointer { .. } => {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    expr.span,
                    "closure function pointer reached LLVM before closure entry materialization",
                ));
            }
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
                self.validate_callee(callee, expr.span);
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

    fn validate_callee(&mut self, callee: &FunctionCallee, span: Span) {
        match callee {
            FunctionCallee::ClosureEntry { state, .. } => {
                self.validate_expr(state);
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    span,
                    "generated closure entry reached LLVM before backend materialization",
                ));
            }
            FunctionCallee::Function(def_id) => self.validate_function_ref(
                *def_id,
                span,
                "backend IR call references missing function",
            ),
            FunctionCallee::FunctionInstance {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
            } => self.validate_function_instance_ref(
                FunctionInstanceRef {
                    def_id: *def_id,
                    arg_module_id: *arg_module_id,
                    self_arg: *self_arg,
                    args,
                    const_args,
                },
                span,
                "backend IR call references missing function instance",
            ),
            FunctionCallee::Method {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
                receiver,
                ..
            } => {
                self.validate_expr(receiver);
                if self_arg.is_none() && args.is_empty() && const_args.is_empty() {
                    self.validate_function_ref(
                        *def_id,
                        span,
                        "backend IR method call references missing function",
                    );
                } else {
                    self.validate_function_instance_ref(
                        FunctionInstanceRef {
                            def_id: *def_id,
                            arg_module_id: *arg_module_id,
                            self_arg: *self_arg,
                            args,
                            const_args,
                        },
                        span,
                        "backend IR method call references missing function instance",
                    );
                }
            }
            FunctionCallee::DynamicTraitMethod {
                object_ty,
                params,
                return_type,
                receiver,
                ..
            } => {
                self.validate_type(*object_ty, span);
                self.validate_type(*return_type, span);
                for param in params {
                    self.validate_runtime_type(*param, span);
                }
                self.validate_expr(receiver);
            }
            FunctionCallee::BuiltinMethod {
                self_ty, receiver, ..
            } => {
                self.validate_type(*self_ty, span);
                self.validate_expr(receiver);
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
            FunctionCallee::Callable(expr) | FunctionCallee::FunctionPointer(expr) => {
                self.validate_expr(expr);
            }
            // Intrinsic value operators are intentionally selected in LLVM codegen; backend
            // lowering only rewrites them when a source-level extension method wins dispatch.
            FunctionCallee::BuiltinOperator(_) => {}
        }
    }

    fn validate_place(&mut self, place: &FunctionPlace) {
        self.validate_type(place.ty, place.span);
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
                    let Some(TyKind::Tuple(elems)) = self.ty_kind(current_ty) else {
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
