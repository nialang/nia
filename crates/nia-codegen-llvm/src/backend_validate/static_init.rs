// SPDX-License-Identifier: GPL-3.0-or-later
use nia_diagnostic::Diagnostic;
use nia_ids::InternedTyId;
use nia_span::Span;
use nia_static_ir::{StaticAddressElem, StaticFieldInit, StaticInit};

use super::BackendValidator;

impl BackendValidator<'_> {
    pub(super) fn validate_static_init(&mut self, ty: InternedTyId, init: &StaticInit, span: Span) {
        match init {
            StaticInit::Zero
            | StaticInit::Int(_)
            | StaticInit::Float(_)
            | StaticInit::Bool(_)
            | StaticInit::Char(_)
            | StaticInit::Byte(_)
            | StaticInit::NullPtr => {}
            StaticInit::Chars(_) | StaticInit::Bytes(_) => {
                if !matches!(self.ty_kind(ty), Some(nia_ty::TyKind::Array { .. })) {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        "I0300",
                        span,
                        "backend IR string static initializer target is not array",
                    ));
                }
            }
            StaticInit::Array(elems) => {
                let Some(elem_ty) = self.array_elem_ty(ty) else {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        "I0300",
                        span,
                        "backend IR array static initializer target is not array",
                    ));
                    return;
                };
                for elem in elems {
                    self.validate_static_init(elem_ty, elem, span);
                }
            }
            StaticInit::Repeat { value, .. } => {
                let Some(elem_ty) = self.array_elem_ty(ty) else {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        "I0300",
                        span,
                        "backend IR repeat static initializer target is not array",
                    ));
                    return;
                };
                self.validate_static_init(elem_ty, value, span);
            }
            StaticInit::Struct(fields) => self.validate_static_struct_init(ty, fields, span),
            StaticInit::AddrOfGlobal { global, path } => {
                let Some(global_item) = self.index.globals.get(global) else {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        "I0300",
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
                        *function,
                        function.module_id,
                        args,
                        span,
                        "backend IR static initializer references missing function instance",
                    );
                }
            }
            StaticInit::StaticArrayPointer {
                array_ty,
                array_init,
            } => {
                if !matches!(self.ty_kind(ty), Some(nia_ty::TyKind::Pointer { .. })) {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        "I0300",
                        span,
                        "backend IR static array pointer initializer target is not pointer",
                    ));
                }
                self.validate_static_init(*array_ty, array_init, span);
            }
        }
    }

    fn validate_static_struct_init(
        &mut self,
        ty: InternedTyId,
        fields: &[StaticFieldInit],
        span: Span,
    ) {
        let Some((def_id, args)) = self.field_base_type(ty) else {
            self.diagnostics.push(Diagnostic::internal_error_at(
                "I0300",
                span,
                "backend IR struct static initializer target is not nominal",
            ));
            return;
        };
        let Some(target_fields) = self.aggregate_fields(def_id, &args) else {
            self.diagnostics.push(Diagnostic::internal_error_at(
                "I0300",
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
                    "I0300",
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
                    "I0300",
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
                            "I0300",
                            span,
                            "backend IR static address path indexes non-array type",
                        ));
                        continue;
                    };
                    current_ty = elem_ty;
                }
                StaticAddressElem::Error => {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        "I0300",
                        span,
                        "backend IR static address path contains invalid element",
                    ));
                }
            }
        }
    }
}
