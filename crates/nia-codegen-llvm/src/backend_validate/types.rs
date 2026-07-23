// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashSet;

use nia_backend_ir::BackendField;
use nia_diagnostic::Diagnostic;
use nia_ids::InternedTyId;
use nia_layout::TypeLayout;
use nia_mangle::mangle_symbol_id;
use nia_span::Span;
use nia_ty::{ArrayLenTy, LayoutBuiltin, PrimitiveTy, RangeTyKind, TyKind, TypeEquivalence};

use super::{BackendValidator, align_to, primitive_layout};

impl BackendValidator<'_> {
    pub(super) fn validate_runtime_type(&mut self, ty: InternedTyId, span: Span) {
        self.validate_type(ty, span);
        if self.layout_of(ty).is_none() {
            let subject = self
                .current_subject
                .map(|subject| format!(" {subject}"))
                .unwrap_or_default();
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                match self.current_item.as_deref() {
                    Some(item) => format!(
                        "backend IR type {ty:?}{subject} in {item} has no ABI layout before LLVM codegen"
                    ),
                    None => format!(
                        "backend IR type {ty:?}{subject} has no ABI layout before LLVM codegen"
                    ),
                },
            ));
        }
    }

    pub(super) fn validate_trait_object_self_type(&mut self, ty: InternedTyId, span: Span) {
        self.validate_type(ty, span);
        if matches!(
            self.index.ty_kind(ty),
            Some(TyKind::SlicePointee { .. } | TyKind::TraitObjectPointee { .. })
        ) {
            return;
        }
        if self.layout_of(ty).is_none() {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!("backend IR trait object self type {ty:?} is not representable"),
            ));
        }
    }

    pub(super) fn validate_type(&mut self, ty: InternedTyId, span: Span) {
        if !self.seen_types.insert(ty) {
            return;
        }
        let Some(kind) = self.index.ty_kind(ty).cloned() else {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                "type belongs to a different compilation session",
            ));
            return;
        };
        match kind {
            TyKind::Pointer { elem, .. }
            | TyKind::VolatilePointer { elem, .. }
            | TyKind::Slice { elem, .. }
            | TyKind::SlicePointee { elem } => {
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
            TyKind::Optional { elem } => self.validate_runtime_type(elem, span),
            TyKind::ErrorUnion { error, value } => {
                self.validate_runtime_type(error, span);
                self.validate_runtime_type(value, span);
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
            TyKind::Nominal { def_id, args, .. } => {
                if self.index.module(def_id.module_id).is_none() {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
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
            }
            | TyKind::TraitObjectPointee {
                trait_args,
                associated_type_bindings,
                ..
            } => {
                for arg in trait_args {
                    self.validate_type(arg, span);
                }
                for binding in associated_type_bindings {
                    for arg in &binding.trait_args {
                        self.validate_type(*arg, span);
                    }
                    self.validate_type(binding.ty, span);
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
            TyKind::ConstOnly => self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!("backend IR type {ty:?} is const-only before LLVM codegen"),
            )),
            TyKind::BuiltinType(builtin) => self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!(
                    "backend IR type {ty:?} is builtin type `{}` before LLVM codegen",
                    builtin.name()
                ),
            )),
            TyKind::SelfParam => self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!(
                    "backend IR type {ty:?} still contains unresolved Self before LLVM codegen"
                ),
            )),
            TyKind::Vector { elem, lanes } => self.validate_vector_type(ty, span, elem, lanes),
            TyKind::Error => {
                let subject = self
                    .current_subject
                    .map(|subject| format!(" {subject}"))
                    .unwrap_or_default();
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    span,
                    match self.current_item.as_deref() {
                        Some(item) => {
                            format!("backend IR type {ty:?}{subject} in {item} is error")
                        }
                        None => format!("backend IR type {ty:?}{subject} is error"),
                    },
                ));
            }
            TyKind::Primitive(_) | TyKind::GenericParam(_) => {}
        }
    }

    fn validate_vector_type(
        &mut self,
        ty: InternedTyId,
        span: Span,
        elem: PrimitiveTy,
        lanes: u32,
    ) {
        if !elem.is_vector_element() {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!(
                    "backend IR SIMD vector type {ty:?} has invalid element type `{}`",
                    elem.name()
                ),
            ));
        }
        if lanes == 0 {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!("backend IR SIMD vector type {ty:?} has zero lanes"),
            ));
        }
    }

    fn validate_array_len(&mut self, len: &ArrayLenTy, span: Span) {
        match len {
            ArrayLenTy::ConstValue(_) => {}
            ArrayLenTy::ConstExpr(id) => {
                let Some(module) = self.index.module(id.module_id) else {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        span,
                        format!(
                            "backend IR array length {id:?} belongs to missing module {:?}",
                            id.module_id
                        ),
                    ));
                    return;
                };
                if !module.const_eval.array_lengths.contains_key(id) {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
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
            ArrayLenTy::GenericParam(name) => {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    span,
                    format!(
                        "backend IR array length const generic `{}` reached LLVM codegen",
                        mangle_symbol_id(*name)
                    ),
                ));
            }
            ArrayLenTy::Infer => {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    span,
                    "backend IR array length inference reached LLVM codegen",
                ));
            }
        }
    }

    fn layout_of(&self, ty: InternedTyId) -> Option<TypeLayout> {
        if let Some(layout) = self.layout_cache.borrow().get(&ty) {
            return layout.clone();
        }
        let layout = self.layout_of_with_active(ty, &mut HashSet::new());
        self.layout_cache.borrow_mut().insert(ty, layout.clone());
        layout
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
        if let Some(layout) = self.index.type_layout(ty) {
            return Some(layout.clone());
        }
        match self.index.ty_kind(ty)? {
            TyKind::Primitive(primitive) => Some(primitive_layout(*primitive)),
            TyKind::Vector { elem, lanes } => self.vector_layout(*elem, *lanes),
            TyKind::Pointer { .. }
            | TyKind::VolatilePointer { .. }
            | TyKind::FunctionPointer { .. } => Some(TypeLayout { size: 8, align: 8 }),
            TyKind::Slice { .. } | TyKind::TraitObject { .. } => {
                Some(TypeLayout { size: 16, align: 8 })
            }
            TyKind::SlicePointee { .. } | TyKind::TraitObjectPointee { .. } => None,
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
            TyKind::Optional { elem } => {
                let elem_layout = self.layout_of_with_active(*elem, active)?;
                Some(tagged_union_layout(&[elem_layout]))
            }
            TyKind::ErrorUnion { error, value } => {
                let error_layout = self.layout_of_with_active(*error, active)?;
                let value_layout = self.layout_of_with_active(*value, active)?;
                Some(tagged_union_layout(&[error_layout, value_layout]))
            }
            TyKind::Nominal {
                def_id,
                args,
                const_args,
            } => {
                self.index.module(def_id.module_id)?;
                if args.is_empty() && const_args.is_empty() {
                    self.index
                        .struct_layout(*def_id)
                        .or_else(|| self.index.union_layout(*def_id))
                        .map(|layout| layout.layout.clone())
                        .or_else(|| {
                            let enum_item = self.index.enum_item(*def_id)?;
                            self.layout_of_with_active(enum_item.backing_type, active)
                        })
                        .or_else(|| {
                            self.index.struct_item(*def_id).and_then(|item| {
                                self.zero_sized_aggregate_layout(&item.fields, active)
                            })
                        })
                        .or_else(|| {
                            self.index.union_item(*def_id).and_then(|item| {
                                self.zero_sized_aggregate_layout(&item.fields, active)
                            })
                        })
                } else {
                    self.index
                        .struct_instance_layout(*def_id, args, const_args)
                        .map(|layout| layout.layout.clone())
                        .or_else(|| {
                            self.index
                                .union_instance_layout(*def_id, args, const_args)
                                .map(|layout| layout.layout.clone())
                        })
                        .or_else(|| {
                            self.index
                                .struct_instance_layouts(*def_id)
                                .find_map(|item| {
                                    (self.same_type_args(&item.key.args, args)
                                        && item.key.const_args.as_slice() == const_args.as_slice())
                                    .then_some(item.layout.layout.clone())
                                })
                        })
                        .or_else(|| {
                            self.index.union_instance_layouts(*def_id).find_map(|item| {
                                (self.same_type_args(&item.key.args, args)
                                    && item.key.const_args.as_slice() == const_args.as_slice())
                                .then_some(item.layout.layout.clone())
                            })
                        })
                        .or_else(|| {
                            self.index
                                .struct_instance(*def_id, args, const_args)
                                .and_then(|item| {
                                    self.zero_sized_aggregate_layout(&item.fields, active)
                                })
                        })
                        .or_else(|| {
                            self.index
                                .struct_instances_for(*def_id)
                                .find(|item| {
                                    self.same_type_args(&item.args, args)
                                        && item.const_args.as_slice() == const_args.as_slice()
                                })
                                .and_then(|item| {
                                    self.zero_sized_aggregate_layout(&item.fields, active)
                                })
                        })
                        .or_else(|| {
                            self.index.struct_item(*def_id).and_then(|item| {
                                self.zero_sized_aggregate_layout(&item.fields, active)
                            })
                        })
                        .or_else(|| {
                            self.index
                                .union_instance(*def_id, args, const_args)
                                .and_then(|item| {
                                    self.zero_sized_aggregate_layout(&item.fields, active)
                                })
                        })
                        .or_else(|| {
                            self.index
                                .union_instances_for(*def_id)
                                .find(|item| {
                                    self.same_type_args(&item.args, args)
                                        && item.const_args.as_slice() == const_args.as_slice()
                                })
                                .and_then(|item| {
                                    self.zero_sized_aggregate_layout(&item.fields, active)
                                })
                        })
                        .or_else(|| {
                            self.index.union_item(*def_id).and_then(|item| {
                                self.zero_sized_aggregate_layout(&item.fields, active)
                            })
                        })
                }
            }
            TyKind::BuiltinTrait { .. } => Some(TypeLayout { size: 0, align: 1 }),
            TyKind::ConstOnly
            | TyKind::BuiltinType(_)
            | TyKind::Projection { .. }
            | TyKind::GenericParam(_)
            | TyKind::SelfParam
            | TyKind::Error => None,
        }
    }

    fn vector_layout(&self, elem: PrimitiveTy, lanes: u32) -> Option<TypeLayout> {
        if !elem.is_vector_element() || lanes == 0 {
            return None;
        }
        let elem_layout = primitive_layout(elem);
        Some(TypeLayout {
            size: elem_layout.size.checked_mul(lanes as u64)?,
            align: elem_layout.align,
        })
    }

    fn zero_sized_aggregate_layout(
        &self,
        fields: &[BackendField],
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
                .and_then(|module| module.const_eval.array_lengths.get(id).copied()),
            ArrayLenTy::Builtin { builtin, ty } => {
                let layout = self.layout_of(*ty)?;
                match builtin {
                    LayoutBuiltin::Size => Some(layout.size),
                    LayoutBuiltin::Align => Some(layout.align),
                }
            }
            ArrayLenTy::GenericParam(_) => None,
            ArrayLenTy::Infer => None,
        }
    }

    pub(super) fn same_type_args(&self, left: &[InternedTyId], right: &[InternedTyId]) -> bool {
        self.same_type_args_for_equiv(left, right)
    }

    pub(super) fn same_optional_type(
        &self,
        left: Option<InternedTyId>,
        right: Option<InternedTyId>,
    ) -> bool {
        match (left, right) {
            (Some(left), Some(right)) => self.same_type(left, right),
            (None, None) => true,
            _ => false,
        }
    }

    pub(super) fn same_type(&self, left: InternedTyId, right: InternedTyId) -> bool {
        if left == right {
            return true;
        }
        if let Some(cached) = self.same_type_cache.borrow().get(&(left, right)) {
            return *cached;
        }
        let same = self.compute_same_type_for_equiv(left, right);
        let mut cache = self.same_type_cache.borrow_mut();
        cache.insert((left, right), same);
        cache.insert((right, left), same);
        same
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

    pub(super) fn ty_kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.index.ty_kind(ty)
    }
}

impl TypeEquivalence for BackendValidator<'_> {
    fn ty_kind_for_equiv(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.ty_kind(ty)
    }

    fn same_array_len_for_equiv(&self, left: &ArrayLenTy, right: &ArrayLenTy) -> bool {
        self.same_array_len(left, right)
    }

    fn same_type_for_equiv(&self, left: InternedTyId, right: InternedTyId) -> bool {
        self.same_type(left, right)
    }
}

fn tagged_union_layout(payloads: &[TypeLayout]) -> TypeLayout {
    let tag_layout = TypeLayout { size: 1, align: 1 };
    let payload_size = payloads.iter().map(|layout| layout.size).max().unwrap_or(0);
    let payload_align = payloads
        .iter()
        .map(|layout| layout.align)
        .max()
        .unwrap_or(1);
    let align = tag_layout.align.max(payload_align);
    let payload_offset = align_to(tag_layout.size, payload_align);
    TypeLayout {
        size: align_to(payload_offset.saturating_add(payload_size), align),
        align,
    }
}
