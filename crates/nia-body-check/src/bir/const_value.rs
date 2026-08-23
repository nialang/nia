// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::Expr;
use nia_body_ir::{
    BuiltinConst, TypedArrayElements, TypedExpr, TypedExprKind, TypedFieldInit, TypedRange,
    TypedSliceRange, TypedUnionRelocation,
};
use nia_span::Span;
use nia_ty::{ArrayLenTy, PrimitiveTy, TyKind};

impl<'a> BodyChecker<'a> {
    // Const evaluation owns values without runtime storage. Materialization
    // rebuilds storage-bearing body IR and preserves frozen allocation
    // provenance wherever a readonly pointer escapes into runtime code.
    pub(super) fn runtime_ty_for_global_const_use(
        &mut self,
        def_id: nia_ids::GlobalDefId,
        fallback: nia_ids::InternedTyId,
    ) -> nia_ids::InternedTyId {
        if def_id.module_id == self.defs.module_id {
            return self
                .typed_runtime_ty_for_current_module_const(def_id)
                .or_else(|| self.const_types.get(&def_id.def_id).copied())
                .unwrap_or(fallback);
        }
        if fallback != self.error() {
            return fallback;
        }
        self.qualified_program_const_type(def_id)
            .unwrap_or(fallback)
    }

    fn typed_runtime_ty_for_current_module_const(
        &mut self,
        def_id: nia_ids::GlobalDefId,
    ) -> Option<nia_ids::InternedTyId> {
        let typed = self
            .const_eval
            .typed_values
            .get(&nia_const_check::ConstKey::Global(def_id))?
            .clone();
        let nia_const_check::ConstValueType::Runtime(ty) = typed.ty else {
            return None;
        };
        Some(ty)
    }

    pub(crate) fn expr_runtime_ty(&mut self, expr: &Expr) -> nia_ids::InternedTyId {
        let ty = self.expr_ty(expr).unwrap_or_else(|| self.error());
        if let Some(def_id) = self.global_const_use(expr) {
            return self.runtime_ty_for_global_const_use(def_id, ty);
        }
        if let Some(def_id) = self.qualified_value(expr)
            && matches!(self.global_def_kind(def_id), Some(nia_defs::DefKind::Const))
        {
            return self.runtime_ty_for_global_const_use(def_id, ty);
        }
        ty
    }

    pub(super) fn lower_const_value_expr(
        &mut self,
        span: Span,
        ty: nia_ids::InternedTyId,
        value: Option<nia_const_check::ConstValue>,
    ) -> TypedExpr {
        self.lower_const_value_expr_with_origin(
            span,
            ty,
            value,
            nia_body_ir::PromotedAllocationId::new(self.defs.module_id, span),
        )
    }

    pub(super) fn lower_const_value_expr_with_origin(
        &mut self,
        span: Span,
        ty: nia_ids::InternedTyId,
        value: Option<nia_const_check::ConstValue>,
        allocation: nia_body_ir::PromotedAllocationId,
    ) -> TypedExpr {
        if ty == self.error() {
            return TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Error,
            };
        }
        match value {
            Some(nia_const_check::ConstValue::Int(value)) => TypedExpr {
                span,
                ty,
                kind: TypedExprKind::BuiltinValue(BuiltinConst::Int(value)),
            },
            Some(nia_const_check::ConstValue::Float(value)) => TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Float(value.to_string()),
            },
            Some(nia_const_check::ConstValue::Bool(value)) => TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Bool(value),
            },
            Some(value) => {
                if let Some(expr) = self.materialize_const_value_expr(span, ty, &value, allocation)
                {
                    return expr;
                }
                self.diagnostics
                    .push(nia_diagnostic::Diagnostic::user_error_at(
                        nia_diagnostic::codes::TYPE_CHECK,
                        span,
                        "runtime expression cannot use this const value",
                    ));
                TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Error,
                }
            }
            None => {
                self.diagnostics
                    .push(nia_diagnostic::Diagnostic::user_error_at(
                        nia_diagnostic::codes::TYPE_CHECK,
                        span,
                        "const value is not available during body check",
                    ));
                TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Error,
                }
            }
        }
    }

    fn materialize_const_value_expr(
        &mut self,
        span: Span,
        ty: nia_ids::InternedTyId,
        value: &nia_const_check::ConstValue,
        fallback_allocation: nia_body_ir::PromotedAllocationId,
    ) -> Option<TypedExpr> {
        let normalized_ty = self.normalization.normalize(ty);
        match self.interner.get(normalized_ty).cloned()? {
            TyKind::Pointer { is_readonly, elem } => {
                if !is_readonly {
                    return None;
                }
                let array = self.materialize_const_array_expr(
                    span,
                    elem,
                    pointer_pointee(value)?,
                    fallback_allocation,
                )?;
                let ty = if array.ty == elem {
                    ty
                } else {
                    self.interner.intern(TyKind::Pointer {
                        is_readonly: true,
                        elem: array.ty,
                    })
                };
                Some(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::StaticArrayPointer {
                        allocation: const_pointer_allocation(value).unwrap_or(fallback_allocation),
                        array: Box::new(array),
                        is_readonly: true,
                    },
                })
            }
            TyKind::Slice { is_readonly, elem } => {
                if !is_readonly {
                    return None;
                }
                // Slice consts are frozen pointers, while the backing storage
                // materializer consumes the pointee array. Keep the pointer
                // itself for provenance and pass only its storage value on.
                let pointee = pointer_pointee(value)?;
                let array_ty = self.const_array_ty_for_slice_value(elem, pointee)?;
                let array = self.materialize_const_array_expr(
                    span,
                    array_ty,
                    pointee,
                    fallback_allocation,
                )?;
                let pointer = TypedExpr {
                    span,
                    ty: self.interner.intern(TyKind::Pointer {
                        is_readonly: true,
                        elem: array.ty,
                    }),
                    kind: TypedExprKind::StaticArrayPointer {
                        allocation: const_pointer_allocation(value).unwrap_or(fallback_allocation),
                        array: Box::new(array),
                        is_readonly: true,
                    },
                };
                Some(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Slice {
                        lhs: Box::new(pointer),
                        range: TypedSliceRange {
                            start: None,
                            end: None,
                            inclusive: false,
                        },
                        is_readonly: true,
                    },
                })
            }
            TyKind::Array { .. } => {
                self.materialize_const_array_expr(span, ty, value, fallback_allocation)
            }
            TyKind::Tuple(elem_tys) => {
                let nia_const_check::ConstValue::Tuple(values) = value else {
                    return None;
                };
                if values.len() != elem_tys.len() {
                    return None;
                }
                Some(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Tuple(
                        values
                            .iter()
                            .cloned()
                            .zip(elem_tys)
                            .map(|(value, elem_ty)| {
                                self.lower_const_value_expr_with_origin(
                                    span,
                                    elem_ty,
                                    Some(value),
                                    fallback_allocation,
                                )
                            })
                            .collect(),
                    ),
                })
            }
            TyKind::Optional { elem } => match value {
                nia_const_check::ConstValue::Optional(None) => Some(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Null,
                }),
                nia_const_check::ConstValue::Optional(Some(value)) => Some(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::OptionalSome {
                        expr: Box::new(self.lower_const_value_expr_with_origin(
                            span,
                            elem,
                            Some((**value).clone()),
                            fallback_allocation,
                        )),
                    },
                }),
                _ => None,
            },
            TyKind::ErrorUnion { error, value: ok } => match value {
                nia_const_check::ConstValue::ErrorUnion(Ok(value)) => Some(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::ErrorOk {
                        expr: Box::new(self.lower_const_value_expr_with_origin(
                            span,
                            ok,
                            Some((**value).clone()),
                            fallback_allocation,
                        )),
                    },
                }),
                nia_const_check::ConstValue::ErrorUnion(Err(value)) => Some(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::ErrorErr {
                        expr: Box::new(self.lower_const_value_expr_with_origin(
                            span,
                            error,
                            Some((**value).clone()),
                            fallback_allocation,
                        )),
                    },
                }),
                _ => None,
            },
            TyKind::Range { bound, .. } => {
                let nia_const_check::ConstValue::Range(range) = value else {
                    return None;
                };
                let lower_bound = |value: nia_ty::IntConst| {
                    Some(Box::new(TypedExpr {
                        span,
                        ty: bound?,
                        kind: TypedExprKind::BuiltinValue(BuiltinConst::Int(value)),
                    }))
                };
                Some(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Range(TypedRange {
                        start: match range.start {
                            Some(value) => lower_bound(value),
                            None => None,
                        },
                        end: match range.end {
                            Some(value) => lower_bound(value),
                            None => None,
                        },
                        inclusive: range.inclusive,
                    }),
                })
            }
            TyKind::Vector { elem, lanes } => {
                let nia_const_check::ConstValue::Vector(values) = value else {
                    return None;
                };
                if values.len() != lanes as usize {
                    return None;
                }
                let lane_ty = self.primitive(elem);
                let first = values.first()?.clone();
                let mut vector = TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Splat {
                        value: Box::new(self.lower_const_value_expr_with_origin(
                            span,
                            lane_ty,
                            Some(first),
                            fallback_allocation,
                        )),
                    },
                };
                let usize_ty = self.primitive(nia_ty::PrimitiveTy::Usize);
                for (index, value) in values.iter().enumerate().skip(1) {
                    vector = TypedExpr {
                        span,
                        ty,
                        kind: TypedExprKind::InsertElement {
                            vector: Box::new(vector),
                            index: Box::new(TypedExpr {
                                span,
                                ty: usize_ty,
                                kind: TypedExprKind::BuiltinValue(BuiltinConst::Int(
                                    nia_ty::IntConst::unsigned(index as u128),
                                )),
                            }),
                            value: Box::new(self.lower_const_value_expr_with_origin(
                                span,
                                lane_ty,
                                Some(value.clone()),
                                fallback_allocation,
                            )),
                        },
                    };
                }
                Some(vector)
            }
            TyKind::Nominal { def_id, .. }
                if !self.is_union_def(def_id)
                    && self.resolved_struct_signature(def_id).is_some() =>
            {
                let nia_const_check::ConstValue::Struct(values) = value else {
                    return None;
                };
                let fields = values
                    .iter()
                    .map(|(name, value)| {
                        let field_ty = self.field_ty_for_aggregate_ty(ty, name)?;
                        Some(TypedFieldInit {
                            field: self.field_def_for_aggregate_ty(ty, name),
                            name: self.symbol_name(*name),
                            value: self.lower_const_value_expr_with_origin(
                                span,
                                field_ty,
                                Some(value.clone()),
                                fallback_allocation,
                            ),
                            span,
                        })
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::StructLiteral { def_id, fields },
                })
            }
            TyKind::Nominal { def_id, .. } if self.is_enum_def(def_id) => {
                self.materialize_const_enum_expr(span, ty, def_id, value, fallback_allocation)
            }
            TyKind::Nominal { def_id, .. } if self.is_union_def(def_id) => {
                let nia_const_check::ConstValue::Union(union) = value else {
                    return None;
                };
                let relocations = union
                    .relocations()
                    .iter()
                    .map(|relocation| {
                        let nia_const_check::ConstPointerValue::Frozen {
                            origin,
                            is_readonly: true,
                            pointee,
                        } = relocation.pointer()
                        else {
                            return None;
                        };
                        let module_id = origin.module_id()?;
                        Some(TypedUnionRelocation {
                            offset: relocation.offset(),
                            width: relocation.width(),
                            allocation: nia_body_ir::PromotedAllocationId::new(
                                module_id,
                                origin.span(),
                            ),
                            pointee: Box::new(self.lower_const_value_expr_with_origin(
                                origin.span(),
                                relocation.pointee(),
                                Some((**pointee).clone()),
                                nia_body_ir::PromotedAllocationId::new(module_id, origin.span()),
                            )),
                        })
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::UnionStorageLiteral {
                        bytes: union
                            .bytes()
                            .iter()
                            .zip(union.initialized())
                            .map(|(byte, initialized)| initialized.then_some(*byte))
                            .collect(),
                        relocations,
                    },
                })
            }
            _ => None,
        }
    }

    fn materialize_const_enum_expr(
        &mut self,
        span: Span,
        ty: nia_ids::InternedTyId,
        enum_id: nia_ids::GlobalDefId,
        value: &nia_const_check::ConstValue,
        fallback_allocation: nia_body_ir::PromotedAllocationId,
    ) -> Option<TypedExpr> {
        let nia_const_check::ConstValue::Enum { variant, payload } = value else {
            return None;
        };
        let (owner, signature) = self.resolved_enum_variant(*variant)?;
        if owner != enum_id {
            return None;
        }
        let fields = match (&signature.payload, payload) {
            (
                nia_item_signatures::EnumVariantPayloadSignature::Unit,
                nia_const_eval::ConstEnumPayload::Unit,
            ) => Vec::new(),
            (
                nia_item_signatures::EnumVariantPayloadSignature::Tuple(field_tys),
                nia_const_eval::ConstEnumPayload::Tuple(values),
            ) if field_tys.len() == values.len() => values
                .iter()
                .cloned()
                .zip(field_tys.iter().copied())
                .map(|(value, field_ty)| {
                    self.lower_const_value_expr_with_origin(
                        span,
                        field_ty,
                        Some(value),
                        fallback_allocation,
                    )
                })
                .collect(),
            (
                nia_item_signatures::EnumVariantPayloadSignature::Named(field_tys),
                nia_const_eval::ConstEnumPayload::Named(values),
            ) if field_tys.len() == values.len() => {
                // Const values store named fields in a map, while enum body IR
                // is positional. Re-establish declaration order here so ABI
                // lowering observes the signature's field layout.
                field_tys
                    .iter()
                    .map(|field| {
                        let value = values.get(&field.name)?.clone();
                        Some(self.lower_const_value_expr_with_origin(
                            span,
                            field.ty,
                            Some(value),
                            fallback_allocation,
                        ))
                    })
                    .collect::<Option<Vec<_>>>()?
            }
            _ => return None,
        };
        Some(TypedExpr {
            span,
            ty,
            kind: TypedExprKind::EnumVariant {
                variant: *variant,
                fields,
            },
        })
    }

    fn const_array_ty_for_slice_value(
        &mut self,
        elem: nia_ids::InternedTyId,
        value: &nia_const_check::ConstValue,
    ) -> Option<nia_ids::InternedTyId> {
        let values = const_array_values(value)?;
        let value_len = u64::try_from(values.len()).ok()?;
        Some(self.interner.intern(TyKind::Array {
            len: ArrayLenTy::ConstValue(value_len),
            elem,
        }))
    }

    fn materialize_const_array_expr(
        &mut self,
        span: Span,
        ty: nia_ids::InternedTyId,
        value: &nia_const_check::ConstValue,
        fallback_allocation: nia_body_ir::PromotedAllocationId,
    ) -> Option<TypedExpr> {
        let normalized_ty = self.normalization.normalize(ty);
        let TyKind::Array { len, elem } = self.interner.get(normalized_ty).cloned()? else {
            return None;
        };
        let values = const_array_values(value)?;
        let value_len = u64::try_from(values.len()).ok()?;
        let ty = match len {
            ArrayLenTy::ConstValue(expected_len) => {
                if expected_len != value_len {
                    return None;
                }
                ty
            }
            ArrayLenTy::Infer => self.interner.intern(TyKind::Array {
                len: ArrayLenTy::ConstValue(value_len),
                elem,
            }),
            ArrayLenTy::GenericParam(_) => return None,
            ArrayLenTy::ConstExpr(_) | ArrayLenTy::Builtin { .. } => {
                if self.array_len_value(span, &len).ok()? != value_len {
                    return None;
                }
                ty
            }
        };
        let elem = self.normalization.normalize(elem);
        match self.interner.get(elem) {
            Some(TyKind::Primitive(PrimitiveTy::U8)) => {
                let bytes = const_ints_to_u8(values)?;
                Some(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::ByteString(bytes),
                })
            }
            Some(TyKind::Primitive(PrimitiveTy::Char)) => {
                let scalars = const_ints_to_u32(values)?;
                if scalars
                    .iter()
                    .all(|scalar| char::from_u32(*scalar).is_some())
                {
                    Some(TypedExpr {
                        span,
                        ty,
                        kind: TypedExprKind::String(scalars),
                    })
                } else {
                    None
                }
            }
            _ => {
                let ConstArrayValues::Values(values) = values else {
                    return None;
                };
                let elems = values
                    .iter()
                    .cloned()
                    .map(|value| {
                        self.lower_const_value_expr_with_origin(
                            span,
                            elem,
                            Some(value),
                            fallback_allocation,
                        )
                    })
                    .collect();
                Some(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::ArrayLiteral {
                        elems: TypedArrayElements::List(elems),
                    },
                })
            }
        }
    }

    pub(crate) fn global_const_value(
        &self,
        def_id: nia_ids::GlobalDefId,
    ) -> Option<nia_const_check::ConstValue> {
        self.global_const_value_for_env(def_id)
    }

    pub(super) fn global_const_allocation(
        &self,
        def_id: nia_ids::GlobalDefId,
        fallback_span: Span,
    ) -> nia_body_ir::PromotedAllocationId {
        let span = self
            .defs_for_module(def_id.module_id)
            .and_then(|defs| defs.as_ref().defs.get(def_id.def_id).map(|def| def.span))
            .unwrap_or(fallback_span);
        nia_body_ir::PromotedAllocationId::new(def_id.module_id, span)
    }

    pub(super) fn local_const_allocation(
        &self,
        local_id: nia_ids::LocalId,
        fallback_span: Span,
    ) -> nia_body_ir::PromotedAllocationId {
        let span = self
            .locals
            .locals
            .get(local_id)
            .map(|local| local.span)
            .unwrap_or(fallback_span);
        nia_body_ir::PromotedAllocationId::new(self.defs.module_id, span)
    }

    fn global_const_value_for_env(
        &self,
        def_id: nia_ids::GlobalDefId,
    ) -> Option<nia_const_check::ConstValue> {
        let key = nia_const_check::ConstKey::Global(def_id);
        if def_id.module_id == self.defs.module_id {
            return self.const_eval.values.get(&key).cloned();
        }
        (self.program_const_values)(def_id.module_id)
            .and_then(|const_eval| const_eval.values.get(&key).cloned())
    }
}

fn pointer_pointee(value: &nia_const_check::ConstValue) -> Option<&nia_const_check::ConstValue> {
    match value {
        nia_const_check::ConstValue::Pointer(nia_const_check::ConstPointerValue::Frozen {
            pointee,
            ..
        }) => Some(pointee),
        nia_const_check::ConstValue::String(_) => Some(value),
        _ => None,
    }
}

fn const_pointer_allocation(
    value: &nia_const_check::ConstValue,
) -> Option<nia_body_ir::PromotedAllocationId> {
    let nia_const_check::ConstValue::Pointer(nia_const_check::ConstPointerValue::Frozen {
        origin,
        is_readonly: true,
        ..
    }) = value
    else {
        return None;
    };
    Some(nia_body_ir::PromotedAllocationId::new(
        origin.module_id()?,
        origin.span(),
    ))
}

enum ConstArrayValues<'a> {
    Values(&'a [nia_const_check::ConstValue]),
    String(&'a str),
}

impl ConstArrayValues<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Values(values) => values.len(),
            Self::String(value) => value.chars().count(),
        }
    }
}

fn const_array_values(value: &nia_const_check::ConstValue) -> Option<ConstArrayValues<'_>> {
    match value {
        nia_const_check::ConstValue::Array(values) => Some(ConstArrayValues::Values(values)),
        nia_const_check::ConstValue::String(value) => Some(ConstArrayValues::String(value)),
        _ => None,
    }
}

fn const_ints_to_u8(values: ConstArrayValues<'_>) -> Option<Vec<u8>> {
    match values {
        ConstArrayValues::Values(values) => values
            .iter()
            .map(|value| {
                let nia_const_check::ConstValue::Int(value) = value else {
                    return None;
                };
                u8::try_from(value.bits()).ok()
            })
            .collect(),
        ConstArrayValues::String(_) => None,
    }
}

fn const_ints_to_u32(values: ConstArrayValues<'_>) -> Option<Vec<u32>> {
    match values {
        ConstArrayValues::Values(values) => values
            .iter()
            .map(|value| {
                let nia_const_check::ConstValue::Int(value) = value else {
                    return None;
                };
                u32::try_from(value.bits()).ok()
            })
            .collect(),
        ConstArrayValues::String(value) => Some(value.chars().map(u32::from).collect()),
    }
}
