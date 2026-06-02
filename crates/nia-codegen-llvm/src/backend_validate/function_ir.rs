// SPDX-License-Identifier: GPL-3.0-or-later
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    FunctionArrayElements, FunctionBody, FunctionCallee, FunctionDeferBody, FunctionExpr,
    FunctionExprKind, FunctionOp, FunctionPlace, FunctionPlaceBase, FunctionPlaceElem,
    FunctionTerminator, validate_function_body,
};
use nia_span::Span;
use nia_ty::{BuiltinTrait, TyKind};

use super::BackendValidator;

impl BackendValidator<'_> {
    pub(super) fn validate_function_body(&mut self, body: &FunctionBody) {
        if let Err(error) = validate_function_body(body) {
            self.diagnostics.push(Diagnostic::error(
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
            self.validate_runtime_type(local.ty, local.span);
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
                self.validate_runtime_type(binding.ty, Span::default());
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
        self.validate_type(expr.ty, expr.span);
        match &expr.kind {
            FunctionExprKind::Global(def_id) => {
                if !self.index.globals.contains_key(def_id) {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        format!("backend IR expression references missing global {def_id:?}"),
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
            FunctionExprKind::FunctionInstance { def_id, args } => {
                self.validate_function_instance_ref(
                    *def_id,
                    args,
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
            FunctionExprKind::InlineAsm(asm) => {
                for input in &asm.inputs {
                    self.validate_expr(&input.value);
                }
                for output in &asm.outputs {
                    self.validate_place(&output.place);
                }
            }
            FunctionExprKind::CStringPointer { array, .. } => self.validate_expr(array),
            FunctionExprKind::ArrayLiteral { elems } => match elems {
                FunctionArrayElements::List(elems) => {
                    for elem in elems {
                        self.validate_expr(elem);
                    }
                }
                FunctionArrayElements::Repeat { value, .. } => self.validate_expr(value),
            },
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
            FunctionExprKind::Unary { expr, .. }
            | FunctionExprKind::Discard(expr)
            | FunctionExprKind::Cast { expr, .. }
            | FunctionExprKind::TraitObjectUpcast { expr, .. }
            | FunctionExprKind::TraitObjectCoercion { expr, .. } => self.validate_expr(expr),
            FunctionExprKind::AddrOf(place) => self.validate_place(place),
            FunctionExprKind::Binary { lhs, rhs, .. } => {
                self.validate_expr(lhs);
                self.validate_expr(rhs);
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
            | FunctionExprKind::Local(_)
            | FunctionExprKind::BuiltinValue(_) => {}
            FunctionExprKind::EnumVariant(def_id) => {
                self.validate_enum_variant_ref(
                    *def_id,
                    expr.span,
                    "backend IR expression references missing enum variant",
                );
            }
        }
    }

    fn validate_callee(&mut self, callee: &FunctionCallee, span: Span) {
        match callee {
            FunctionCallee::Function(def_id) => self.validate_function_ref(
                *def_id,
                span,
                "backend IR call references missing function",
            ),
            FunctionCallee::FunctionInstance { def_id, args } => self
                .validate_function_instance_ref(
                    *def_id,
                    args,
                    span,
                    "backend IR call references missing function instance",
                ),
            FunctionCallee::Method {
                def_id,
                args,
                receiver,
            } => {
                self.validate_expr(receiver);
                if args.is_empty() {
                    self.validate_function_ref(
                        *def_id,
                        span,
                        "backend IR method call references missing function",
                    );
                } else {
                    self.validate_function_instance_ref(
                        *def_id,
                        args,
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
                if !matches!(trait_id, BuiltinTrait::GetPtrConst | BuiltinTrait::GetPtr) {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!(
                            "backend IR call contains unresolved builtin place method {trait_id:?}::{method:?}"
                        ),
                    ));
                }
            }
            FunctionCallee::TraitMethod {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
                receiver,
            } => {
                self.validate_type(*self_ty, span);
                for arg in trait_args.iter().chain(args) {
                    self.validate_type(*arg, span);
                }
                self.validate_expr(receiver);
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "backend IR call contains unresolved trait method `{method_name}` {method_id:?} on trait {trait_id:?}"
                    ),
                ));
            }
            FunctionCallee::FunctionPointer(expr) => self.validate_expr(expr),
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
                    self.diagnostics.push(Diagnostic::error(
                        place.span,
                        format!("backend IR place references missing local {local_id:?}"),
                    ));
                }
            }
            FunctionPlaceBase::Global(def_id) => {
                if !self.index.globals.contains_key(def_id) {
                    self.diagnostics.push(Diagnostic::error(
                        place.span,
                        format!("backend IR place references missing global {def_id:?}"),
                    ));
                }
            }
            FunctionPlaceBase::Deref(expr) => self.validate_expr(expr),
            FunctionPlaceBase::Error => {}
        }
        for elem in &place.elems {
            match elem {
                FunctionPlaceElem::Index(expr) => self.validate_expr(expr),
                FunctionPlaceElem::Field(_) => {
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
                    if let Some(TyKind::Pointer { elem, .. }) = self.ty_kind(current_ty) {
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
}
