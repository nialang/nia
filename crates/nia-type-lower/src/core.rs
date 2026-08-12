// SPDX-License-Identifier: GPL-3.0-or-later
//! Core type-syntax lowering for paths, aggregates, pointers, ranges, and callables.

use super::*;

impl TypeLowerer<'_, '_> {
    pub(crate) fn lower_type_in_context(
        &mut self,
        ty: &TypeRef,
        context: TypeContext,
    ) -> InternedTyId {
        let lowered = self.lower_type(ty, context);
        self.type_uses.insert(ty.node_key.site().clone(), lowered);
        if matches!(self.type_store.get(lowered), Some(TyKind::Opaque))
            && context != TypeContext::Pointee
        {
            // `opaque` is only meaningful behind a direct pointer. Reject it at the first value
            // boundary so incomplete layout cannot leak into later type phases.
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                ty.span,
                "`opaque` is incomplete and may only be used as a direct pointer target",
            ));
        }
        if context == TypeContext::Value
            && let Some(message) = self.invalid_value_type_message(lowered)
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                ty.span,
                message,
            ));
        }
        lowered
    }

    pub(crate) fn lower_type(&mut self, ty: &TypeRef, context: TypeContext) -> InternedTyId {
        match &ty.kind {
            TypeKind::Error => self.append.intern(TyKind::Error),
            TypeKind::Infer => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_NORMALIZATION,
                    ty.span,
                    "`_` type inference is not valid in this type lowering context",
                ));
                self.append.intern(TyKind::Error)
            }
            TypeKind::Opaque => self.append.intern(TyKind::Opaque),
            TypeKind::Never => self.append.intern(TyKind::Primitive(PrimitiveTy::Never)),
            TypeKind::Tuple { elems } => {
                let elems = elems
                    .iter()
                    .map(|elem| self.lower_type_in_context(elem, TypeContext::Value))
                    .collect();
                self.append.intern(TyKind::Tuple(elems))
            }
            TypeKind::Optional { elem } => {
                let elem = self.lower_type_in_context(elem, TypeContext::Value);
                self.append.intern(TyKind::Optional { elem })
            }
            TypeKind::ErrorUnion { error, value } => {
                let error = self.lower_type_in_context(error, TypeContext::Value);
                let value = self.lower_type_in_context(value, TypeContext::Value);
                self.append.intern(TyKind::ErrorUnion { error, value })
            }
            TypeKind::SelfType => self.self_type_stack.last().copied().unwrap_or_else(|| {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_NORMALIZATION,
                    ty.span,
                    "`Self` is only valid in traits and extend blocks",
                ));
                self.append.intern(TyKind::Error)
            }),
            TypeKind::Pointer { is_readonly, elem } => {
                if let TypeKind::Callable {
                    params,
                    return_type,
                } = &elem.kind
                {
                    let (params, return_type) =
                        self.lower_callable_signature(params, return_type.as_deref());
                    self.append.intern(TyKind::Callable {
                        is_readonly: *is_readonly,
                        params,
                        return_type,
                    })
                } else if let Some(trait_object) = self.lower_trait_object_type(*is_readonly, elem)
                {
                    trait_object
                } else {
                    let elem = self.lower_type_in_context(elem, TypeContext::Pointee);
                    self.append.intern(TyKind::Pointer {
                        is_readonly: *is_readonly,
                        elem,
                    })
                }
            }
            TypeKind::VolatilePointer { is_readonly, elem } => {
                let elem = self.lower_type_in_context(elem, TypeContext::Pointee);
                self.append.intern(TyKind::VolatilePointer {
                    is_readonly: *is_readonly,
                    elem,
                })
            }
            TypeKind::Slice { is_readonly, elem } => {
                let elem = self.lower_type_in_context(elem, TypeContext::Value);
                self.append.intern(TyKind::Slice {
                    is_readonly: *is_readonly,
                    elem,
                })
            }
            TypeKind::SlicePointee { elem } => {
                let elem = self.lower_type_in_context(elem, TypeContext::Value);
                self.append.intern(TyKind::SlicePointee { elem })
            }
            TypeKind::Array { len, elem } => {
                let len = self.lower_array_len(len);
                let elem = self.lower_type_in_context(elem, TypeContext::Value);
                self.append.intern(TyKind::Array { len, elem })
            }
            TypeKind::Range {
                start,
                end,
                inclusive,
            } => self.lower_range_type(ty.span, start.as_deref(), end.as_deref(), *inclusive),
            TypeKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            } => {
                let params = params
                    .iter()
                    .map(|param| self.lower_type_in_context(param, TypeContext::Value))
                    .collect();
                let return_type = match return_type {
                    Some(return_type) => {
                        self.lower_type_in_context(return_type, TypeContext::Return)
                    }
                    None => self.append.intern(TyKind::Tuple(Vec::new())),
                };
                self.append.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic: *is_variadic,
                })
            }
            TypeKind::Callable {
                params,
                return_type,
            } => {
                let (params, return_type) =
                    self.lower_callable_signature(params, return_type.as_deref());
                self.append.intern(TyKind::CallablePointee {
                    params,
                    return_type,
                })
            }
            TypeKind::Path { segments } => {
                let Some(first) = segments.first() else {
                    return self.append.intern(TyKind::Error);
                };
                let Some(type_segment) = type_name_segment(segments) else {
                    return self.append.intern(TyKind::Error);
                };
                match self
                    .resolved
                    .node_type_names
                    .get(ty.node_key.site())
                    .copied()
                {
                    Some(TypeNameResolution::Primitive(primitive)) => {
                        self.lower_primitive_type(primitive)
                    }
                    Some(TypeNameResolution::BuiltinTrait(trait_id)) => self
                        .lower_builtin_trait_or_extend_target_type(
                            ty.span,
                            type_segment,
                            trait_id,
                            context,
                        ),
                    Some(TypeNameResolution::GenericParam) => {
                        let Some(name) = type_path_segment_name(first) else {
                            return self.append.intern(TyKind::Error);
                        };
                        self.append.intern(TyKind::GenericParam(*name))
                    }
                    Some(TypeNameResolution::AssociatedType) => {
                        let Some(name) = type_path_segment_name(first) else {
                            return self.append.intern(TyKind::Error);
                        };
                        self.lower_scoped_associated_type(ty.span, name, type_segment)
                    }
                    Some(TypeNameResolution::Def(def_id)) => {
                        let def_id = self
                            .resolved
                            .node_qualified_type_names
                            .get(ty.node_key.site())
                            .copied()
                            .unwrap_or(GlobalDefId {
                                module_id: self.module_id,
                                def_id,
                            });
                        self.lower_path_type(ty.span, type_segment, def_id, context)
                    }
                    Some(TypeNameResolution::External(global_id)) => {
                        self.lower_path_type(ty.span, type_segment, global_id, context)
                    }
                    Some(TypeNameResolution::Error) | None => self.append.intern(TyKind::Error),
                }
            }
            TypeKind::Projection {
                ty,
                trait_ref,
                name,
            } => {
                let self_ty = self.lower_type_in_context(ty, TypeContext::Value);
                let trait_ty = self.lower_type_in_context(trait_ref, TypeContext::TraitBound);
                let trait_ty = self.normalize_if_known(trait_ty);
                let Some((trait_id, args, const_args)) = self.projection_trait_id(trait_ty) else {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_NORMALIZATION,
                        trait_ref.span,
                        "projection trait must resolve to a trait",
                    ));
                    return self.append.intern(TyKind::Error);
                };
                if !self.trait_id_has_associated_type(trait_id, name) {
                    let name = self.symbol_name(*name);
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_NORMALIZATION,
                        ty.span,
                        format!("trait does not define associated type `{name}`"),
                    ));
                    return self.append.intern(TyKind::Error);
                }
                self.append.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args: args,
                    trait_const_args: const_args,
                    name: *name,
                })
            }
        }
    }

    pub(crate) fn lower_range_type(
        &mut self,
        span: Span,
        start: Option<&TypeRef>,
        end: Option<&TypeRef>,
        inclusive: bool,
    ) -> InternedTyId {
        let start_ty = start.map(|ty| self.lower_type_in_context(ty, TypeContext::Value));
        let end_ty = end.map(|ty| self.lower_type_in_context(ty, TypeContext::Value));
        let kind = match (start_ty, end_ty, inclusive) {
            (Some(_), Some(_), false) => RangeTyKind::Exclusive,
            (Some(_), Some(_), true) => RangeTyKind::Inclusive,
            (Some(_), None, false) => RangeTyKind::From,
            (None, Some(_), false) => RangeTyKind::To,
            (None, Some(_), true) => RangeTyKind::ToInclusive,
            (None, None, false) => RangeTyKind::Full,
            (Some(_), None, true) | (None, None, true) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_NORMALIZATION,
                    span,
                    "inclusive range type requires an end bound",
                ));
                return self.append.intern(TyKind::Error);
            }
        };
        let bound = match (start_ty, end_ty) {
            (Some(start_ty), Some(end_ty)) => {
                if !self.types_equivalent(start_ty, end_ty) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_NORMALIZATION,
                        span,
                        "range type bounds must have the same type",
                    ));
                    return self.append.intern(TyKind::Error);
                }
                Some(start_ty)
            }
            (Some(bound), None) | (None, Some(bound)) => Some(bound),
            (None, None) => None,
        };
        if let Some(bound) = bound
            && !self.can_be_integer(bound)
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                span,
                "range bound type must be an integer type",
            ));
            return self.append.intern(TyKind::Error);
        }
        self.append.intern(TyKind::Range { kind, bound })
    }

    pub(crate) fn normalize_if_known(&self, ty: InternedTyId) -> InternedTyId {
        ty
    }

    pub(crate) fn lower_path_type(
        &mut self,
        span: Span,
        segment: &TypePathSegment,
        def_id: GlobalDefId,
        context: TypeContext,
    ) -> InternedTyId {
        let mut args = Vec::new();
        let mut const_args = Vec::new();
        let mut seen_assoc_bindings = HashSet::new();
        let mut seen_assoc_binding = false;
        let generic_params = self.generic_params_for_def(def_id).unwrap_or_default();
        let mut positional_index = 0usize;
        for arg in &segment.args {
            match arg {
                TypeArg::Type(arg_ty) => {
                    if seen_assoc_binding {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            arg_ty.span,
                            "positional type arguments must precede associated type bindings",
                        ));
                    }
                    match generic_params
                        .get(positional_index)
                        .map(|generic| &generic.kind)
                    {
                        Some(GenericParamKind::Const { ty }) => {
                            let Some(value) = self.lower_const_generic_value_from_type_ref(arg_ty)
                            else {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::TYPE_NORMALIZATION,
                                    arg_ty.span,
                                    "expected const generic argument",
                                ));
                                positional_index += 1;
                                continue;
                            };
                            let ty = self.lower_generic_param_type(def_id.module_id, ty);
                            const_args.push(ConstGenericArg { ty, value });
                        }
                        _ => args.push(self.lower_type_or_const_type_arg(arg_ty)),
                    }
                    positional_index += 1;
                }
                TypeArg::Const(expr) => {
                    match generic_params
                        .get(positional_index)
                        .map(|generic| &generic.kind)
                    {
                        Some(GenericParamKind::Const { ty }) => {
                            let Some(value) = self.lower_const_generic_value_from_expr(expr) else {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::TYPE_NORMALIZATION,
                                    expr.span,
                                    "unsupported const generic argument",
                                ));
                                positional_index += 1;
                                continue;
                            };
                            let ty = self.lower_generic_param_type(def_id.module_id, ty);
                            const_args.push(ConstGenericArg { ty, value });
                        }
                        _ => {
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::TYPE_NORMALIZATION,
                                expr.span,
                                "const value generic argument supplied for type parameter",
                            ));
                        }
                    }
                    positional_index += 1;
                }
                TypeArg::TypeOrConst { ty: arg_ty, expr } => {
                    if seen_assoc_binding {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            arg_ty.span,
                            "positional type arguments must precede associated type bindings",
                        ));
                    }
                    match generic_params
                        .get(positional_index)
                        .map(|generic| &generic.kind)
                    {
                        Some(GenericParamKind::Const { ty }) => {
                            let Some(value) = self.lower_const_generic_value_from_expr(expr) else {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::TYPE_NORMALIZATION,
                                    expr.span,
                                    "unsupported const generic argument",
                                ));
                                positional_index += 1;
                                continue;
                            };
                            let ty = self.lower_generic_param_type(def_id.module_id, ty);
                            const_args.push(ConstGenericArg { ty, value });
                        }
                        _ => args.push(self.lower_type_in_context(arg_ty, TypeContext::Value)),
                    }
                    positional_index += 1;
                }
                TypeArg::AssocBinding {
                    key,
                    span,
                    ty: binding_ty,
                } => {
                    seen_assoc_binding = true;
                    if context == TypeContext::TraitBound {
                        self.lower_type_in_context(binding_ty, TypeContext::Value);
                        if !self.is_trait_def(def_id) {
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::TYPE_NORMALIZATION,
                                *span,
                                "associated type bindings require a trait bound",
                            ));
                        } else {
                            let Some(LoweredAssocBindingKey { name, .. }) =
                                self.lower_assoc_binding_key(key, Some(TraitId::Source(def_id)))
                            else {
                                continue;
                            };
                            if !seen_assoc_bindings.insert(self.assoc_binding_seen_key(
                                name,
                                None,
                                &[],
                                &[],
                            )) {
                                let name = self.symbol_name(*name);
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::TYPE_NORMALIZATION,
                                    *span,
                                    format!("duplicate associated type binding `{name}`"),
                                ));
                            }
                            if !self.trait_has_associated_type(def_id, name) {
                                let name = self.symbol_name(*name);
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::TYPE_NORMALIZATION,
                                    *span,
                                    format!("trait does not define associated type `{name}`"),
                                ));
                            }
                        }
                    } else {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            *span,
                            "associated type bindings are only valid in trait bounds",
                        ));
                    }
                }
            }
        }
        self.check_type_arg_count(span, def_id, positional_index);
        if context == TypeContext::ExtendTarget && self.is_trait_def(def_id) {
            let object_args = self
                .lower_trait_object_args(span, segment, TraitId::Source(def_id))
                .unwrap_or_default();
            return self.append.intern(TyKind::TraitObjectPointee {
                trait_id: TraitId::Source(def_id),
                trait_args: object_args.trait_args,
                trait_const_args: object_args.trait_const_args,
                associated_type_bindings: object_args.associated_type_bindings,
            });
        }
        self.append.intern(TyKind::Nominal {
            def_id,
            args,
            const_args,
        })
    }

    pub(crate) fn lower_type_or_const_type_arg(&mut self, ty: &TypeRef) -> InternedTyId {
        if matches!(ty.kind, TypeKind::Path { .. })
            && !self
                .resolved
                .node_type_names
                .contains_key(ty.node_key.site())
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                ty.span,
                "expected type generic argument",
            ));
            return self.append.intern(TyKind::Error);
        }
        self.lower_type_in_context(ty, TypeContext::Value)
    }
}
