// SPDX-License-Identifier: GPL-3.0-or-later
//! Place traversal and assignment validation contracts.

use super::*;

impl BackendValidator<'_> {
    pub(super) fn validate_place(
        &mut self,
        place: &FunctionPlace,
    ) -> Option<nia_ids::InternedTyId> {
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

    pub(super) fn validate_assignment(
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

    pub(super) fn place_is_writable(&self, place: &FunctionPlace) -> bool {
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

    pub(super) fn invalid_projection(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR projection has an invalid contract: {message}"),
        ));
    }

    pub(super) fn validate_addr_of_result(
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

    pub(super) fn const_generic_value_name(&self, value: &ConstGenericValue) -> String {
        match value {
            ConstGenericValue::GenericParam(name) => mangle_symbol_id(*name),
            ConstGenericValue::ConstExpr(id) => format!("{id:?}"),
            ConstGenericValue::Int(value) => value.bits().to_string(),
            ConstGenericValue::Bool(value) => value.to_string(),
            ConstGenericValue::Char(value) => value.to_string(),
        }
    }
}
