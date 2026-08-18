// SPDX-License-Identifier: GPL-3.0-or-later
use nia_diagnostic::Diagnostic;
use nia_ids::InternedTyId;
use nia_span::Span;
use nia_static_ir::{StaticAddressElem, StaticFieldInit, StaticInit};
use nia_ty::{PrimitiveTy, TyKind};

use super::{BackendValidator, FunctionInstanceRef};
use crate::literals::parse_float_literal;

impl BackendValidator<'_> {
    pub(super) fn validate_static_init(&mut self, ty: InternedTyId, init: &StaticInit, span: Span) {
        match init {
            StaticInit::Zero => {}
            StaticInit::Int(_) => {
                if !matches!(
                    self.ty_kind(ty),
                    Some(TyKind::Primitive(primitive))
                        if primitive.is_integer()
                            || matches!(*primitive, PrimitiveTy::Bool | PrimitiveTy::Char)
                ) {
                    self.invalid_static_scalar(
                        ty,
                        span,
                        "integer initializer target is not integer-like",
                    );
                }
            }
            StaticInit::Float(text) => {
                let valid_type = matches!(
                    self.ty_kind(ty),
                    Some(TyKind::Primitive(PrimitiveTy::F32 | PrimitiveTy::F64))
                );
                let valid_value = parse_float_literal(text).is_some_and(|value| {
                    value.is_finite()
                        && (matches!(self.ty_kind(ty), Some(TyKind::Primitive(PrimitiveTy::F64)))
                            || (value as f32).is_finite())
                });
                if !valid_type {
                    self.invalid_static_scalar(
                        ty,
                        span,
                        "float initializer target is not f32 or f64",
                    );
                } else if !valid_value {
                    self.invalid_static_scalar(
                        ty,
                        span,
                        "float initializer spelling or range is invalid",
                    );
                }
            }
            StaticInit::Bool(_) => {
                if !matches!(self.ty_kind(ty), Some(TyKind::Primitive(PrimitiveTy::Bool))) {
                    self.invalid_static_scalar(ty, span, "bool initializer target is not bool");
                }
            }
            StaticInit::Char(value) => {
                if !matches!(self.ty_kind(ty), Some(TyKind::Primitive(PrimitiveTy::Char))) {
                    self.invalid_static_scalar(ty, span, "char initializer target is not char");
                }
                if char::from_u32(*value).is_none() {
                    self.invalid_static_scalar(
                        ty,
                        span,
                        "char initializer is not a Unicode scalar",
                    );
                }
            }
            StaticInit::Byte(_) => {
                if !matches!(self.ty_kind(ty), Some(TyKind::Primitive(PrimitiveTy::U8))) {
                    self.invalid_static_scalar(ty, span, "byte initializer target is not u8");
                }
            }
            StaticInit::NullPtr => {
                if !matches!(
                    self.ty_kind(ty),
                    Some(TyKind::Pointer { .. } | TyKind::FunctionPointer { .. })
                ) {
                    self.invalid_static_scalar(
                        ty,
                        span,
                        "null pointer initializer target is not a pointer",
                    );
                }
            }
            StaticInit::Chars(values) => {
                if matches!(
                    self.ty_kind(ty),
                    Some(TyKind::Array { elem, .. })
                        if matches!(self.ty_kind(*elem), Some(TyKind::Primitive(PrimitiveTy::Char)))
                ) {
                    self.validate_static_array_len(ty, values.len(), span, "char string");
                    if values.iter().any(|value| char::from_u32(*value).is_none()) {
                        self.invalid_static_scalar(
                            ty,
                            span,
                            "char string contains an invalid Unicode scalar",
                        );
                    }
                } else {
                    self.invalid_static_scalar(
                        ty,
                        span,
                        "char string initializer target is not char array",
                    );
                }
            }
            StaticInit::Bytes(values) => {
                if matches!(
                    self.ty_kind(ty),
                    Some(TyKind::Array { elem, .. })
                        if matches!(self.ty_kind(*elem), Some(TyKind::Primitive(PrimitiveTy::U8)))
                ) {
                    self.validate_static_array_len(ty, values.len(), span, "byte string");
                } else {
                    self.invalid_static_scalar(
                        ty,
                        span,
                        "byte string initializer target is not u8 array",
                    );
                }
            }
            StaticInit::Array(elems) => {
                let Some(elem_ty) = self.array_elem_ty(ty) else {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        span,
                        "backend IR array static initializer target is not array",
                    ));
                    return;
                };
                self.validate_static_array_len(ty, elems.len(), span, "array");
                for elem in elems {
                    self.validate_static_init(elem_ty, elem, span);
                }
            }
            StaticInit::Repeat { value, count } => {
                let Some(elem_ty) = self.array_elem_ty(ty) else {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        span,
                        "backend IR repeat static initializer target is not array",
                    ));
                    return;
                };
                self.validate_static_array_count(ty, *count, span, "repeat");
                self.validate_static_init(elem_ty, value, span);
            }
            StaticInit::Struct(fields) => self.validate_static_struct_init(ty, fields, span),
            StaticInit::AddrOfGlobal { global, path } => {
                let Some(global_item) = self.index.global(*global) else {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        span,
                        format!(
                            "backend IR static initializer references missing global {global:?}"
                        ),
                    ));
                    return;
                };
                self.validate_static_address_path(global_item.ty, path, span);
            }
            StaticInit::AddrOfFunction { function, args } => {
                if args.is_empty() {
                    self.validate_function_ref(
                        *function,
                        span,
                        "backend IR static initializer references missing function",
                    );
                } else {
                    self.validate_function_instance_ref(
                        FunctionInstanceRef {
                            def_id: *function,
                            arg_module_id: function.module_id,
                            self_arg: None,
                            args,
                            const_args: &[],
                        },
                        span,
                        "backend IR static initializer references missing function instance",
                    );
                }
            }
        }
    }

    fn invalid_static_scalar(&mut self, _ty: InternedTyId, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR static initializer has an invalid scalar contract: {message}"),
        ));
    }

    fn validate_static_array_len(
        &mut self,
        ty: InternedTyId,
        actual: usize,
        span: Span,
        kind: &'static str,
    ) {
        let Ok(actual) = u64::try_from(actual) else {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!("backend IR {kind} static initializer length exceeds u64"),
            ));
            return;
        };
        self.validate_static_array_count(ty, actual, span, kind);
    }

    fn validate_static_array_count(
        &mut self,
        ty: InternedTyId,
        actual: u64,
        span: Span,
        kind: &'static str,
    ) {
        let Some(nia_ty::TyKind::Array { len, .. }) = self.ty_kind(ty) else {
            return;
        };
        let Some(expected) = self.array_len_value(len) else {
            return;
        };
        if actual != expected {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!(
                    "backend IR {kind} static initializer has {actual} elements but its array type requires {expected}"
                ),
            ));
        }
    }

    fn validate_static_struct_init(
        &mut self,
        ty: InternedTyId,
        fields: &[StaticFieldInit],
        span: Span,
    ) {
        let Some((def_id, args, const_args)) = self.field_base_type(ty) else {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                "backend IR struct static initializer target is not nominal",
            ));
            return;
        };
        let Some(target_fields) = self.aggregate_fields(def_id, &args, &const_args) else {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!(
                    "backend IR struct static initializer references missing aggregate {def_id:?}"
                ),
            ));
            return;
        };
        let target_fields = target_fields
            .iter()
            .map(|field| (field.def_id, field.ty))
            .collect::<Vec<_>>();
        for init in fields {
            let Some(field_id) = init.field else {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    span,
                    "backend IR static initializer has invalid field",
                ));
                continue;
            };
            let Some((_, field_ty)) = target_fields
                .iter()
                .find(|(candidate, _)| *candidate == field_id)
            else {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    span,
                    format!("backend IR static initializer references missing field {field_id:?}"),
                ));
                continue;
            };
            self.validate_static_init(*field_ty, &init.value, span);
        }
    }

    fn validate_static_address_path(
        &mut self,
        mut current_ty: InternedTyId,
        path: &[StaticAddressElem],
        span: Span,
    ) {
        for elem in path {
            match elem {
                StaticAddressElem::Field(field) => {
                    if let Some(field_ty) = self.validate_aggregate_field(
                        current_ty,
                        *field,
                        span,
                        "backend IR static address path references missing field",
                    ) {
                        current_ty = field_ty;
                    }
                }
                StaticAddressElem::Index(_) => {
                    let Some(elem_ty) = self.array_elem_ty(current_ty) else {
                        self.diagnostics.push(Diagnostic::internal_error_at(
                            nia_diagnostic::codes::INVALID_BACKEND_IR,
                            span,
                            "backend IR static address path indexes non-array type",
                        ));
                        continue;
                    };
                    current_ty = elem_ty;
                }
                StaticAddressElem::Error => {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        span,
                        "backend IR static address path contains invalid element",
                    ));
                }
            }
        }
    }
}
