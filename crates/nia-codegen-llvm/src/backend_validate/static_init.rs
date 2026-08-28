// SPDX-License-Identifier: GPL-3.0-or-later
use nia_diagnostic::Diagnostic;
use nia_ids::InternedTyId;
use nia_span::Span;
use nia_static_ir::{StaticAddressElem, StaticFieldInit, StaticInit};
use nia_ty::{IntConst, PrimitiveTy, TyKind};

use super::{BackendValidator, FunctionInstanceRef};
use crate::literals::parse_float_literal;

impl BackendValidator<'_> {
    pub(super) fn validate_static_init(&mut self, ty: InternedTyId, init: &StaticInit, span: Span) {
        match init {
            StaticInit::Zero => {}
            StaticInit::Int(value) => self.validate_static_integer(ty, *value, span),
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
            StaticInit::Tuple(elems) => {
                let Some(TyKind::Tuple(expected)) = self.ty_kind(ty).cloned() else {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        span,
                        "backend IR tuple static initializer target is not tuple",
                    ));
                    return;
                };
                if expected.len() != elems.len() {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        span,
                        format!(
                            "backend IR tuple static initializer has {} elements but its tuple type requires {}",
                            elems.len(),
                            expected.len()
                        ),
                    ));
                }
                for (elem_ty, elem) in expected.into_iter().zip(elems) {
                    self.validate_static_init(elem_ty, elem, span);
                }
            }
            StaticInit::Vector(lanes) => {
                let Some(TyKind::Vector {
                    elem,
                    lanes: expected,
                }) = self.ty_kind(ty).cloned()
                else {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        span,
                        "backend IR vector static initializer target is not vector",
                    ));
                    return;
                };
                if usize::try_from(expected).ok() != Some(lanes.len()) {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        span,
                        format!(
                            "backend IR vector static initializer has {} lanes but its vector type requires {expected}",
                            lanes.len()
                        ),
                    ));
                }
                for lane in lanes {
                    self.validate_static_vector_lane(elem, lane, span);
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
                self.validate_static_global_address(
                    ty,
                    global_item.is_let,
                    global_item.ty,
                    path,
                    span,
                );
            }
            StaticInit::AddrOfFunction {
                function,
                args,
                const_args,
            } => {
                self.validate_static_function_address_signature(
                    ty, *function, args, const_args, span,
                );
                if args.is_empty() && const_args.is_empty() {
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
                            const_args,
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

    fn validate_static_integer(&mut self, ty: InternedTyId, value: IntConst, span: Span) {
        let Some(TyKind::Primitive(primitive)) = self.ty_kind(ty) else {
            self.invalid_static_scalar(ty, span, "integer initializer target is not integer-like");
            return;
        };
        if !self.static_integer_fits(*primitive, value) {
            self.invalid_static_scalar(
                ty,
                span,
                "integer initializer value is outside its target type",
            );
        }
    }

    fn static_integer_fits(&self, primitive: PrimitiveTy, value: IntConst) -> bool {
        let pointer_width = self
            .target
            .pointer_size
            .checked_mul(8)
            .and_then(|bits| u32::try_from(bits).ok())
            .unwrap_or(0);
        match primitive {
            PrimitiveTy::Bool => !value.is_signed() && matches!(value.bits(), 0 | 1),
            PrimitiveTy::Char => {
                !value.is_signed()
                    && u32::try_from(value.bits())
                        .ok()
                        .and_then(char::from_u32)
                        .is_some()
            }
            primitive if primitive.is_integer() => {
                value.fits_primitive_int(primitive, pointer_width)
            }
            _ => false,
        }
    }

    fn validate_static_vector_lane(&mut self, elem: PrimitiveTy, lane: &StaticInit, span: Span) {
        let valid = match lane {
            StaticInit::Zero => true,
            StaticInit::Int(value) => self.static_integer_fits(elem, *value),
            StaticInit::Float(text) => {
                matches!(elem, PrimitiveTy::F32 | PrimitiveTy::F64)
                    && parse_float_literal(text).is_some_and(|value| {
                        value.is_finite()
                            && (elem == PrimitiveTy::F64 || (value as f32).is_finite())
                    })
            }
            StaticInit::Bool(_) => elem == PrimitiveTy::Bool,
            StaticInit::Char(value) => {
                elem == PrimitiveTy::Char && char::from_u32(*value).is_some()
            }
            StaticInit::Byte(_) => elem == PrimitiveTy::U8,
            StaticInit::Chars(_)
            | StaticInit::Bytes(_)
            | StaticInit::Array(_)
            | StaticInit::Tuple(_)
            | StaticInit::Vector(_)
            | StaticInit::Repeat { .. }
            | StaticInit::Struct(_)
            | StaticInit::NullPtr
            | StaticInit::AddrOfGlobal { .. }
            | StaticInit::AddrOfFunction { .. } => false,
        };
        if !valid {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                "backend IR vector static initializer lane does not match its primitive element type",
            ));
        }
    }

    fn validate_static_global_address(
        &mut self,
        target_ty: InternedTyId,
        source_is_let: bool,
        source_ty: InternedTyId,
        path: &[StaticAddressElem],
        span: Span,
    ) {
        let Some(TyKind::Pointer {
            is_readonly,
            elem: target_elem,
        }) = self.ty_kind(target_ty).cloned()
        else {
            self.invalid_static_address(
                target_ty,
                span,
                "global address target is not a data pointer",
            );
            return;
        };
        if source_is_let && !is_readonly {
            self.invalid_static_address(
                target_ty,
                span,
                "global address exposes immutable storage as mutable",
            );
        }
        let Some(source_elem) = self.validate_static_address_path(source_ty, path, span) else {
            return;
        };
        if !self.same_type(target_elem, source_elem) {
            self.invalid_static_address(
                target_ty,
                span,
                "global address pointee type does not match its path",
            );
        }
    }

    fn invalid_static_address(&mut self, _ty: InternedTyId, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR static address initializer has an invalid contract: {message}"),
        ));
    }

    /// Checks the source-visible ABI before a static function address is
    /// bitcast into its destination global. Function-pointer identity carries
    /// parameter, return, and variadic facts that an LLVM pointer cast cannot
    /// recover after emission.
    fn validate_static_function_address_signature(
        &mut self,
        ty: InternedTyId,
        function: nia_ids::GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
        span: Span,
    ) {
        let Some(TyKind::FunctionPointer {
            params: target_params,
            return_type: target_return,
            is_variadic: target_variadic,
        }) = self.ty_kind(ty).cloned()
        else {
            self.invalid_static_address(
                ty,
                span,
                "function address target is not a function pointer",
            );
            return;
        };

        let signature = if args.is_empty() {
            self.index.function(function).map(|item| {
                (
                    item.params
                        .iter()
                        .map(|param| param.local_ty)
                        .collect::<Vec<_>>(),
                    item.return_type,
                    item.is_variadic,
                )
            })
        } else {
            self.index
                .function_instance(function, function.module_id, None, args, const_args)
                .or_else(|| {
                    self.index.function_instances_for(function).find(|item| {
                        item.self_arg.is_none()
                            && self.same_type_args(&item.args, args)
                            && self.same_const_args(&item.const_args, const_args)
                    })
                })
                .map(|item| {
                    (
                        item.params
                            .iter()
                            .map(|param| param.local_ty)
                            .collect::<Vec<_>>(),
                        item.return_type,
                        item.is_variadic,
                    )
                })
        };
        let Some((actual_params, actual_return, actual_variadic)) = signature else {
            return;
        };
        if actual_params.len() != target_params.len()
            || actual_params
                .iter()
                .zip(&target_params)
                .any(|(actual, target)| !self.same_type(*actual, *target))
        {
            self.invalid_static_address(
                ty,
                span,
                "function address parameter types do not match its target",
            );
        }
        if !self.same_type(actual_return, target_return) {
            self.invalid_static_address(
                ty,
                span,
                "function address return type does not match its target",
            );
        }
        if actual_variadic != target_variadic {
            self.invalid_static_address(
                ty,
                span,
                "function address variadic flag does not match its target",
            );
        }
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
        let is_union = self.index.union_item(def_id).is_some();
        if is_union && fields.len() != 1 {
            self.invalid_static_aggregate(
                span,
                "union static initializer must initialize exactly one field",
            );
        }
        let mut seen_fields = Vec::new();
        for init in fields {
            let Some(field_id) = init.field else {
                self.invalid_static_aggregate(span, "static initializer has invalid field");
                continue;
            };
            let Some((_, field_ty)) = target_fields
                .iter()
                .find(|(candidate, _)| *candidate == field_id)
            else {
                self.invalid_static_aggregate(span, "static initializer references missing field");
                continue;
            };
            if seen_fields.contains(&field_id) {
                self.invalid_static_aggregate(span, "struct static initializer duplicates a field");
            } else {
                seen_fields.push(field_id);
            }
            self.validate_static_init(*field_ty, &init.value, span);
        }
        if !is_union {
            for (field_id, _) in &target_fields {
                if !seen_fields.contains(field_id) {
                    self.invalid_static_aggregate(
                        span,
                        "struct static initializer is missing a field",
                    );
                }
            }
        }
    }

    fn invalid_static_aggregate(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR static aggregate initializer has an invalid contract: {message}"),
        ));
    }

    fn validate_static_address_path(
        &mut self,
        mut current_ty: InternedTyId,
        path: &[StaticAddressElem],
        span: Span,
    ) -> Option<InternedTyId> {
        for elem in path {
            match elem {
                StaticAddressElem::Field(field) => {
                    let field_ty = self.validate_aggregate_field(
                        current_ty,
                        *field,
                        span,
                        "backend IR static address path references missing field",
                    )?;
                    current_ty = field_ty;
                }
                StaticAddressElem::Index(_) => {
                    let Some(elem_ty) = self.array_elem_ty(current_ty) else {
                        self.diagnostics.push(Diagnostic::internal_error_at(
                            nia_diagnostic::codes::INVALID_BACKEND_IR,
                            span,
                            "backend IR static address path indexes non-array type",
                        ));
                        return None;
                    };
                    current_ty = elem_ty;
                }
                StaticAddressElem::Error => {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        span,
                        "backend IR static address path contains invalid element",
                    ));
                    return None;
                }
            }
        }
        Some(current_ty)
    }
}
