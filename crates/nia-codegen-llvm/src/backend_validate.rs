// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashSet;

use crate::program_index::ProgramIndex;
use nia_backend_ir::{BackendModule, BackendProgram, BackendTraitObjectVtableFunction};
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    FunctionArrayElements, FunctionBody, FunctionCallee, FunctionDeferBody, FunctionExpr,
    FunctionExprKind, FunctionOp, FunctionPlace, FunctionPlaceBase, FunctionPlaceElem,
    FunctionTerminator,
};
use nia_ids::{GlobalDefId, InternedTyId};
use nia_layout::TypeLayout;
use nia_span::Span;
use nia_ty::{ArrayLenTy, LayoutBuiltin, PrimitiveTy, RangeTyKind, TyKind};

pub(super) fn validate_backend_program(
    program: &BackendProgram,
    index: &ProgramIndex<'_>,
) -> Vec<Diagnostic> {
    let mut validator = BackendValidator {
        index,
        diagnostics: Vec::new(),
        seen_types: HashSet::new(),
    };
    for module in &program.modules {
        validator.validate_module(module);
    }
    validator.diagnostics
}

struct BackendValidator<'a> {
    index: &'a ProgramIndex<'a>,
    diagnostics: Vec<Diagnostic>,
    seen_types: HashSet<InternedTyId>,
}

impl BackendValidator<'_> {
    fn validate_module(&mut self, module: &BackendModule) {
        for function in &module.functions {
            if function.generics.is_empty() {
                self.validate_type(function.return_type, function.span);
                for param in &function.params {
                    self.validate_runtime_type(param.ty, param.span);
                }
                if let Some(body) = &function.function_body {
                    self.validate_function_body(body);
                }
            }
        }
        for function in &module.function_instances {
            self.validate_type(function.return_type, function.span);
            for param in &function.params {
                self.validate_runtime_type(param.ty, param.span);
            }
            if let Some(body) = &function.function_body {
                self.validate_function_body(body);
            }
        }
        for global in &module.globals {
            self.validate_runtime_type(global.ty, global.span);
        }
        for item in &module.structs {
            if item.generics.is_empty() {
                for field in &item.fields {
                    self.validate_runtime_type(field.ty, field.span);
                }
            }
        }
        for item in &module.struct_instances {
            for field in &item.fields {
                self.validate_runtime_type(field.ty, field.span);
            }
        }
        for item in &module.unions {
            if item.generics.is_empty() {
                for field in &item.fields {
                    self.validate_runtime_type(field.ty, field.span);
                }
            }
        }
        for item in &module.union_instances {
            for field in &item.fields {
                self.validate_runtime_type(field.ty, field.span);
            }
        }
        for item in &module.enums {
            self.validate_runtime_type(item.backing_type, item.span);
        }
        for vtable in &module.trait_object_vtables {
            self.validate_runtime_type(vtable.key.self_ty, vtable.span);
            self.validate_runtime_type(vtable.key.object_ty, vtable.span);
            for entry in &vtable.entries {
                match &entry.function {
                    BackendTraitObjectVtableFunction::Function(def_id) => {
                        self.validate_function_ref(
                            *def_id,
                            vtable.span,
                            "backend IR vtable references missing function",
                        );
                    }
                    BackendTraitObjectVtableFunction::FunctionInstance { def_id, args } => {
                        self.validate_function_instance_ref(
                            *def_id,
                            args,
                            vtable.span,
                            "backend IR vtable references missing function instance",
                        );
                    }
                }
            }
        }
    }

    fn validate_function_body(&mut self, body: &FunctionBody) {
        self.validate_type(body.ty, body.span);
        for local in &body.locals {
            self.validate_runtime_type(local.ty, local.span);
        }
        for block in &body.blocks {
            for op in &block.ops {
                self.validate_op(op);
            }
            self.validate_terminator(&block.terminator);
        }
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
                nia_function_ir::FunctionForHeader::CStyle { cond } => {
                    if let Some(cond) = cond {
                        self.validate_expr(cond);
                    }
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
            FunctionExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    self.validate_expr(&field.value);
                }
            }
            FunctionExprKind::UnionLiteral { field, .. } => self.validate_expr(&field.value),
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
            FunctionExprKind::Field { lhs, .. } => self.validate_expr(lhs),
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
            | FunctionExprKind::EnumVariant(_)
            | FunctionExprKind::BuiltinValue(_) => {}
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
            FunctionCallee::BuiltinPlaceMethod {
                self_ty,
                trait_args,
                receiver,
                ..
            } => {
                self.validate_type(*self_ty, span);
                for arg in trait_args {
                    self.validate_type(*arg, span);
                }
                self.validate_expr(receiver);
            }
            FunctionCallee::TraitMethod {
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
            }
            FunctionCallee::FunctionPointer(expr) => self.validate_expr(expr),
            FunctionCallee::BuiltinOperator(_) => {}
        }
    }

    fn validate_place(&mut self, place: &FunctionPlace) {
        self.validate_type(place.ty, place.span);
        match &place.base {
            FunctionPlaceBase::Global(def_id) => {
                if !self.index.globals.contains_key(def_id) {
                    self.diagnostics.push(Diagnostic::error(
                        place.span,
                        format!("backend IR place references missing global {def_id:?}"),
                    ));
                }
            }
            FunctionPlaceBase::Deref(expr) => self.validate_expr(expr),
            FunctionPlaceBase::Local(_) | FunctionPlaceBase::Error => {}
        }
        for elem in &place.elems {
            match elem {
                FunctionPlaceElem::Index(expr) => self.validate_expr(expr),
                FunctionPlaceElem::Field(_) | FunctionPlaceElem::Error => {}
            }
        }
    }

    fn validate_function_ref(&mut self, def_id: GlobalDefId, span: Span, message: &str) {
        if !self.index.functions.contains_key(&def_id) {
            self.diagnostics
                .push(Diagnostic::error(span, format!("{message} {def_id:?}")));
        }
    }

    fn validate_function_instance_ref(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        span: Span,
        message: &str,
    ) {
        for arg in args {
            self.validate_type(*arg, span);
        }
        let exists =
            self.index
                .function_instances
                .keys()
                .any(|(candidate_def, _, candidate_args)| {
                    *candidate_def == def_id && self.same_type_args(candidate_args, args)
                });
        if !exists {
            self.diagnostics
                .push(Diagnostic::error(span, format!("{message} {def_id:?}")));
        }
    }

    fn validate_runtime_type(&mut self, ty: InternedTyId, span: Span) {
        self.validate_type(ty, span);
        if self.layout_of(ty).is_none() {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("backend IR type {ty:?} has no ABI layout before LLVM codegen"),
            ));
        }
    }

    fn validate_type(&mut self, ty: InternedTyId, span: Span) {
        if !self.seen_types.insert(ty) {
            return;
        }
        let Some(module) = self.index.module(ty.interner_id) else {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "backend IR type {ty:?} belongs to missing module {:?}",
                    ty.interner_id
                ),
            ));
            return;
        };
        let Some(kind) = module.interner.get(ty).cloned() else {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("backend IR type {ty:?} is missing from its owner interner"),
            ));
            return;
        };
        match kind {
            TyKind::Pointer { elem, .. } | TyKind::Slice { elem, .. } => {
                self.validate_type(elem, span);
            }
            TyKind::Array { len, elem } => {
                self.validate_array_len(&len, span);
                self.validate_runtime_type(elem, span);
            }
            TyKind::Range { bound, .. } => {
                if let Some(bound) = bound {
                    self.validate_runtime_type(bound, span);
                }
            }
            TyKind::FunctionPointer {
                params,
                return_type,
                ..
            } => {
                for param in params {
                    self.validate_runtime_type(param, span);
                }
                self.validate_type(return_type, span);
            }
            TyKind::Nominal { def_id, args } => {
                if self.index.module(def_id.module_id).is_none() {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!(
                            "backend IR nominal type {def_id:?} belongs to missing module {:?}",
                            def_id.module_id
                        ),
                    ));
                }
                for arg in args {
                    self.validate_type(arg, span);
                }
            }
            TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            } => {
                for arg in trait_args {
                    self.validate_type(arg, span);
                }
                for (_, ty) in associated_type_bindings {
                    self.validate_type(ty, span);
                }
            }
            TyKind::Projection {
                self_ty,
                trait_args,
                ..
            } => {
                self.validate_type(self_ty, span);
                for arg in trait_args {
                    self.validate_type(arg, span);
                }
            }
            TyKind::BuiltinTrait { args, .. } => {
                for arg in args {
                    self.validate_type(arg, span);
                }
            }
            TyKind::Primitive(_) | TyKind::GenericParam(_) | TyKind::Error => {}
        }
    }

    fn validate_array_len(&mut self, len: &ArrayLenTy, span: Span) {
        match len {
            ArrayLenTy::ConstValue(_) => {}
            ArrayLenTy::ConstExpr(id) => {
                let Some(module) = self.index.module(id.module_id) else {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!(
                            "backend IR array length {id:?} belongs to missing module {:?}",
                            id.module_id
                        ),
                    ));
                    return;
                };
                if !module.comptime.array_lengths.contains_key(id) {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!(
                            "backend IR array length {id:?} was not evaluated before LLVM codegen"
                        ),
                    ));
                }
            }
            ArrayLenTy::Builtin { ty, .. } => {
                self.validate_runtime_type(*ty, span);
            }
            ArrayLenTy::Infer => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    "backend IR array length inference reached LLVM codegen",
                ));
            }
        }
    }

    fn layout_of(&self, ty: InternedTyId) -> Option<TypeLayout> {
        self.layout_of_with_active(ty, &mut HashSet::new())
    }

    fn layout_of_with_active(
        &self,
        ty: InternedTyId,
        active: &mut HashSet<InternedTyId>,
    ) -> Option<TypeLayout> {
        if !active.insert(ty) {
            return None;
        }
        let layout = self.layout_of_inner(ty, active);
        active.remove(&ty);
        layout
    }

    fn layout_of_inner(
        &self,
        ty: InternedTyId,
        active: &mut HashSet<InternedTyId>,
    ) -> Option<TypeLayout> {
        let owner = self.index.module(ty.interner_id)?;
        if let Some(layout) = owner
            .layouts
            .types
            .iter()
            .find_map(|(candidate, layout)| (*candidate == ty).then_some(layout.clone()))
        {
            return Some(layout);
        }
        match owner.interner.get(ty)? {
            TyKind::Primitive(primitive) => Some(primitive_layout(*primitive)),
            TyKind::Pointer { .. } | TyKind::FunctionPointer { .. } => {
                Some(TypeLayout { size: 8, align: 8 })
            }
            TyKind::Slice { .. } | TyKind::TraitObject { .. } => {
                Some(TypeLayout { size: 16, align: 8 })
            }
            TyKind::Range { bound: None, .. } => Some(TypeLayout { size: 0, align: 1 }),
            TyKind::Range {
                kind,
                bound: Some(bound),
            } => {
                let field_count = match kind {
                    RangeTyKind::Exclusive | RangeTyKind::Inclusive => 2,
                    RangeTyKind::From | RangeTyKind::To | RangeTyKind::ToInclusive => 1,
                    RangeTyKind::Full => 0,
                };
                let bound_layout = self.layout_of_with_active(*bound, active)?;
                Some(TypeLayout {
                    size: align_to(
                        bound_layout.size.saturating_mul(field_count),
                        bound_layout.align,
                    ),
                    align: bound_layout.align,
                })
            }
            TyKind::Array { len, elem } => {
                let len = self.array_len_value(len)?;
                let elem_layout = self.layout_of_with_active(*elem, active)?;
                Some(TypeLayout {
                    size: elem_layout.size.saturating_mul(len),
                    align: elem_layout.align,
                })
            }
            TyKind::Nominal { def_id, args } => {
                let def_owner = self.index.module(def_id.module_id)?;
                if args.is_empty() {
                    def_owner
                        .layouts
                        .structs
                        .iter()
                        .find_map(|(candidate, layout)| {
                            (*candidate == *def_id).then_some(layout.layout.clone())
                        })
                        .or_else(|| {
                            def_owner
                                .layouts
                                .unions
                                .iter()
                                .find_map(|(candidate, layout)| {
                                    (*candidate == *def_id).then_some(layout.layout.clone())
                                })
                        })
                        .or_else(|| {
                            self.index.structs.get(def_id).and_then(|item| {
                                self.zero_sized_aggregate_layout(&item.fields, active)
                            })
                        })
                        .or_else(|| {
                            self.index.unions.get(def_id).and_then(|item| {
                                self.zero_sized_aggregate_layout(&item.fields, active)
                            })
                        })
                } else {
                    def_owner
                        .layouts
                        .struct_instances
                        .iter()
                        .find_map(|(key, layout)| {
                            (key.def_id == *def_id && self.same_type_args(&key.args, args))
                                .then_some(layout.layout.clone())
                        })
                        .or_else(|| {
                            def_owner
                                .layouts
                                .union_instances
                                .iter()
                                .find_map(|(key, layout)| {
                                    (key.def_id == *def_id && self.same_type_args(&key.args, args))
                                        .then_some(layout.layout.clone())
                                })
                        })
                        .or_else(|| {
                            self.index
                                .struct_instances
                                .iter()
                                .find(|((candidate_def, candidate_args), _)| {
                                    *candidate_def == *def_id
                                        && self.same_type_args(candidate_args, args)
                                })
                                .and_then(|(_, item)| {
                                    self.zero_sized_aggregate_layout(&item.fields, active)
                                })
                        })
                        .or_else(|| {
                            self.index.structs.get(def_id).and_then(|item| {
                                self.zero_sized_aggregate_layout(&item.fields, active)
                            })
                        })
                        .or_else(|| {
                            self.index
                                .union_instances
                                .iter()
                                .find(|((candidate_def, candidate_args), _)| {
                                    *candidate_def == *def_id
                                        && self.same_type_args(candidate_args, args)
                                })
                                .and_then(|(_, item)| {
                                    self.zero_sized_aggregate_layout(&item.fields, active)
                                })
                        })
                        .or_else(|| {
                            self.index.unions.get(def_id).and_then(|item| {
                                self.zero_sized_aggregate_layout(&item.fields, active)
                            })
                        })
                }
            }
            TyKind::BuiltinTrait { .. } => Some(TypeLayout { size: 0, align: 1 }),
            TyKind::Projection { .. } | TyKind::GenericParam(_) | TyKind::Error => None,
        }
    }

    fn zero_sized_aggregate_layout(
        &self,
        fields: &[nia_backend_ir::BackendField],
        active: &mut HashSet<InternedTyId>,
    ) -> Option<TypeLayout> {
        for field in fields {
            let field_layout = self.layout_of_with_active(field.ty, active)?;
            if field_layout.size != 0 {
                return None;
            }
        }
        Some(TypeLayout { size: 0, align: 1 })
    }

    fn array_len_value(&self, len: &ArrayLenTy) -> Option<u64> {
        match len {
            ArrayLenTy::ConstValue(value) => Some(*value),
            ArrayLenTy::ConstExpr(id) => self
                .index
                .module(id.module_id)
                .and_then(|module| module.comptime.array_lengths.get(id).copied()),
            ArrayLenTy::Builtin { builtin, ty } => {
                let layout = self.layout_of(*ty)?;
                match builtin {
                    LayoutBuiltin::Size => Some(layout.size),
                    LayoutBuiltin::Align => Some(layout.align),
                }
            }
            ArrayLenTy::Infer => None,
        }
    }

    fn same_type_args(&self, left: &[InternedTyId], right: &[InternedTyId]) -> bool {
        left.len() == right.len()
            && left
                .iter()
                .zip(right)
                .all(|(left, right)| self.same_type(*left, *right))
    }

    fn same_type(&self, left: InternedTyId, right: InternedTyId) -> bool {
        if left == right {
            return true;
        }
        match (self.ty_kind(left), self.ty_kind(right)) {
            (Some(TyKind::Primitive(left)), Some(TyKind::Primitive(right))) => left == right,
            (
                Some(TyKind::Pointer {
                    is_const: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::Pointer {
                    is_const: right_const,
                    elem: right_elem,
                }),
            )
            | (
                Some(TyKind::Slice {
                    is_const: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::Slice {
                    is_const: right_const,
                    elem: right_elem,
                }),
            ) => left_const == right_const && self.same_type(*left_elem, *right_elem),
            (
                Some(TyKind::Array {
                    len: left_len,
                    elem: left_elem,
                }),
                Some(TyKind::Array {
                    len: right_len,
                    elem: right_elem,
                }),
            ) => {
                self.same_array_len(left_len, right_len) && self.same_type(*left_elem, *right_elem)
            }
            (
                Some(TyKind::Nominal {
                    def_id: left_def,
                    args: left_args,
                }),
                Some(TyKind::Nominal {
                    def_id: right_def,
                    args: right_args,
                }),
            ) => left_def == right_def && self.same_type_args(left_args, right_args),
            _ => false,
        }
    }

    fn same_array_len(&self, left: &ArrayLenTy, right: &ArrayLenTy) -> bool {
        match (left, right) {
            (ArrayLenTy::Infer, ArrayLenTy::Infer) => true,
            (ArrayLenTy::ConstValue(left), ArrayLenTy::ConstValue(right)) => left == right,
            (ArrayLenTy::ConstValue(left), ArrayLenTy::ConstExpr(right))
            | (ArrayLenTy::ConstExpr(right), ArrayLenTy::ConstValue(left)) => self
                .array_len_value(&ArrayLenTy::ConstExpr(*right))
                .is_some_and(|right| *left == right),
            (ArrayLenTy::ConstExpr(left), ArrayLenTy::ConstExpr(right)) => {
                left == right || {
                    let left = self.array_len_value(&ArrayLenTy::ConstExpr(*left));
                    let right = self.array_len_value(&ArrayLenTy::ConstExpr(*right));
                    left.is_some() && left == right
                }
            }
            (
                ArrayLenTy::Builtin {
                    builtin: left_builtin,
                    ty: left_ty,
                },
                ArrayLenTy::Builtin {
                    builtin: right_builtin,
                    ty: right_ty,
                },
            ) => left_builtin == right_builtin && self.same_type(*left_ty, *right_ty),
            _ => false,
        }
    }

    fn ty_kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.index.module(ty.interner_id)?.interner.get(ty)
    }
}

fn primitive_layout(primitive: PrimitiveTy) -> TypeLayout {
    let (size, align) = match primitive {
        PrimitiveTy::I8 | PrimitiveTy::U8 | PrimitiveTy::Bool => (1, 1),
        PrimitiveTy::I16 | PrimitiveTy::U16 => (2, 2),
        PrimitiveTy::I32 | PrimitiveTy::U32 | PrimitiveTy::F32 | PrimitiveTy::Char => (4, 4),
        PrimitiveTy::I64
        | PrimitiveTy::U64
        | PrimitiveTy::F64
        | PrimitiveTy::Isize
        | PrimitiveTy::Usize => (8, 8),
        PrimitiveTy::I128 | PrimitiveTy::U128 => (16, 16),
        PrimitiveTy::Void | PrimitiveTy::Never => (0, 1),
    };
    TypeLayout { size, align }
}

fn align_to(value: u64, align: u64) -> u64 {
    if align == 0 {
        return value;
    }
    value.div_ceil(align) * align
}
