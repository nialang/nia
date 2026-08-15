use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConstTypeCompatibility {
    Mismatch,
    Unknown,
}

impl Analyzer<'_> {
    pub(super) fn substitute_ty_generics(&mut self, ty: InternedTyId) -> InternedTyId {
        let module_id = self.current_execution_module_id();
        // Imported const queries install their execution frame before they
        // necessarily touch that module's type interner. Substitution owns
        // this prerequisite because every caller needs the destination
        // interner, even when only a nested const generic changes.
        self.ensure_type_context(module_id)
            .expect("current execution module must have a type context");
        let mut type_substitutions = SymbolMap::default();
        let mut const_substitutions = SymbolMap::default();
        let frames = self.active_execution_frames().collect::<Vec<_>>();
        for frame in frames.into_iter().rev() {
            type_substitutions.extend(frame.type_substitutions.clone());
            const_substitutions.extend(frame.const_substitutions.clone());
        }
        let interner = self
            .type_contexts
            .get(&module_id)
            .expect("type context must exist for current execution module");
        nia_ty::substitute_ty(
            interner.store,
            &interner.append,
            ty,
            &|name| type_substitutions.get(name).copied(),
            &|name| const_substitutions.get(name).cloned(),
            None,
        )
    }

    pub(super) fn instantiate_resolved_function_generics(
        &mut self,
        span: Span,
        input: ConstFunctionInstantiationInput<'_>,
    ) -> Result<ConstGenericInstantiation, ConstError> {
        let ConstFunctionInstantiationInput {
            signature_module_id,
            signature,
            generic_args,
            arg_exprs,
            expected_return,
            initial,
        } = input;
        if self.ensure_type_context(signature_module_id).is_none() {
            return Err(ConstError {
                span,
                message: "cannot instantiate const function without module type interner"
                    .to_string(),
            });
        }
        if !generic_args.is_empty()
            && let ArityCheck::Mismatch { actual, .. } =
                check_exact_arity(signature.generic_params.len(), generic_args.len())
        {
            return Err(ConstError {
                span,
                message: format!(
                    "generic argument count mismatch for const function: expected {}, got {}",
                    signature.generic_params.len(),
                    actual
                ),
            });
        }
        let mut substitutions = initial.type_substitutions;
        let mut const_substitutions = initial.const_substitutions;
        if generic_args.is_empty() {
            if let Some(expected) = expected_return
                && let Some(expected) = self.type_for_module_or_none(expected, signature_module_id)
            {
                self.infer_generic_substitutions_from_tys(
                    span,
                    signature_module_id,
                    signature.return_type,
                    expected,
                    &mut substitutions,
                    &mut const_substitutions,
                )?;
            }
            for (param, arg_expr) in signature.params.iter().zip(arg_exprs) {
                let expected = self.const_expected_param_type(
                    signature_module_id,
                    param.ty,
                    &substitutions,
                    &const_substitutions,
                );
                let concrete_expected =
                    expected.filter(|expected| !self.type_contains_generic(*expected));
                let Some(arg_ty) =
                    self.resolved_const_arg_runtime_type(arg_expr, concrete_expected)
                else {
                    if concrete_expected.is_some_and(|expected| {
                        self.resolved_const_arg_expected_compatibility(arg_expr, expected)
                            == ConstTypeCompatibility::Mismatch
                    }) {
                        return Err(ConstError {
                            span: arg_expr.span(),
                            message: match arg_expr.kind() {
                                ResolvedConstExprKind::Match(_) => {
                                    "const match expression does not match expected type"
                                }
                                _ => "const call argument does not match expected type",
                            }
                            .to_string(),
                        });
                    }
                    continue;
                };
                self.infer_generic_substitutions_from_tys(
                    span,
                    signature_module_id,
                    param.ty,
                    arg_ty,
                    &mut substitutions,
                    &mut const_substitutions,
                )?;
            }
            for generic in &signature.generic_params {
                let inferred = match generic.kind {
                    GenericParamSignatureKind::Type => substitutions.contains_key(&generic.name),
                    GenericParamSignatureKind::Const { .. } => {
                        const_substitutions.contains_key(&generic.name)
                    }
                };
                if inferred {
                    continue;
                }
                let name = self.symbol_name(generic.name);
                let message = match generic.kind {
                    GenericParamSignatureKind::Type => {
                        format!("cannot infer const generic type argument `{name}`")
                    }
                    GenericParamSignatureKind::Const { .. } => {
                        format!("cannot infer const generic argument `{name}`")
                    }
                };
                return Err(ConstError { span, message });
            }
        } else {
            for (generic, arg) in signature.generic_params.iter().zip(generic_args) {
                match (&generic.kind, arg) {
                    (GenericParamSignatureKind::Type, ResolvedConstGenericArg::Type(arg)) => {
                        let canonical = self.type_for_module(arg.ty(), signature_module_id)?;
                        substitutions.insert(generic.name, canonical);
                    }
                    (
                        GenericParamSignatureKind::Const { ty },
                        ResolvedConstGenericArg::Const(expr),
                    ) => {
                        let value = self.const_generic_arg_from_resolved_expr(
                            expr,
                            *ty,
                            signature_module_id,
                        )?;
                        const_substitutions
                            .insert(generic.name, nia_ty::ConstGenericArg { ty: *ty, value });
                    }
                    (GenericParamSignatureKind::Type, _) => {
                        let name = self.symbol_name(generic.name);
                        return Err(ConstError {
                            span: arg.span(),
                            message: format!("generic argument `{name}` must be a type"),
                        });
                    }
                    (GenericParamSignatureKind::Const { .. }, _) => {
                        let name = self.symbol_name(generic.name);
                        return Err(ConstError {
                            span: arg.span(),
                            message: format!("generic argument `{name}` must be a const value"),
                        });
                    }
                }
            }
        }
        Ok(ConstGenericInstantiation {
            type_substitutions: substitutions,
            const_substitutions,
        })
    }

    fn const_generic_arg_from_resolved_expr(
        &mut self,
        expr: &ResolvedConstExpr,
        expected_ty: InternedTyId,
        module_id: ModuleId,
    ) -> Result<nia_ty::ConstGenericValue, ConstError> {
        let expected_ty = self.type_for_module(expected_ty, module_id)?;
        let expected = self
            .type_contexts
            .get(&module_id)
            .and_then(|interner| interner.get(expected_ty))
            .cloned();
        let value = nia_const_eval::eval_resolved_const_expr(expr, self)?;
        match (expected, value) {
            (Some(TyKind::Primitive(PrimitiveTy::Bool)), ConstValue::Bool(value)) => {
                Ok(nia_ty::ConstGenericValue::Bool(value))
            }
            (Some(TyKind::Primitive(PrimitiveTy::Char)), ConstValue::Int(value)) => {
                let scalar = u32::try_from(value.bits()).ok().and_then(char::from_u32);
                scalar
                    .map(nia_ty::ConstGenericValue::Char)
                    .ok_or_else(|| ConstError {
                        span: expr.span(),
                        message: "const generic character argument is not a valid Unicode scalar"
                            .to_string(),
                    })
            }
            (Some(TyKind::Primitive(primitive)), ConstValue::Int(value))
                if primitive_integer_layout(primitive, self.input.target.pointer_width)
                    .is_some() =>
            {
                let (min, max) =
                    primitive_integer_range_for_target(primitive, self.input.target.pointer_width)
                        .expect("integer primitive must have a target range");
                if !int_const_in_i128_range(value, min, max) {
                    return Err(ConstError {
                        span: expr.span(),
                        message:
                            "const generic integer argument is out of range for parameter type"
                                .to_string(),
                    });
                }
                Ok(nia_ty::ConstGenericValue::Int(value))
            }
            _ => Err(ConstError {
                span: expr.span(),
                message: "const generic argument does not match parameter type".to_string(),
            }),
        }
    }

    pub(super) fn const_expected_param_type(
        &mut self,
        module_id: ModuleId,
        ty: InternedTyId,
        type_substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> Option<InternedTyId> {
        self.ensure_type_context(module_id)?;
        let types = self.type_contexts.get(&module_id)?;
        Some(nia_ty::substitute_ty(
            types.store,
            &types.append,
            ty,
            &|generic| type_substitutions.get(generic).copied(),
            &|generic| const_substitutions.get(generic).cloned(),
            None,
        ))
    }

    pub(super) fn resolved_const_arg_runtime_type(
        &mut self,
        expr: &ResolvedConstExpr,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let ty = self.resolved_const_expr_type(expr, expected)?;
        match ty {
            ConstValueType::Runtime(ty) => Some(ty),
            ConstValueType::Array {
                elem,
                len: Some(len),
            } => {
                let elem = elem.runtime()?;
                self.const_runtime_type(
                    elem,
                    |elem| TyKind::Array {
                        len: ArrayLenTy::ConstValue(len),
                        elem,
                    },
                    self.current_execution_module_id(),
                )
            }
            ConstValueType::Int
            | ConstValueType::Bool
            | ConstValueType::String
            | ConstValueType::Array { len: None, .. } => None,
        }
    }

    fn resolved_const_arg_expected_compatibility(
        &mut self,
        expr: &ResolvedConstExpr,
        expected: InternedTyId,
    ) -> ConstTypeCompatibility {
        match expr.kind() {
            ResolvedConstExprKind::Cast { expr: inner, ty } => {
                match self.resolved_const_cast_expected_compatibility(inner, *ty) {
                    ConstTypeCompatibility::Mismatch => ConstTypeCompatibility::Mismatch,
                    ConstTypeCompatibility::Unknown => ConstTypeCompatibility::Unknown,
                }
            }
            ResolvedConstExprKind::Match(matched)
                if self.resolved_const_match_has_definite_pattern_mismatch(matched) =>
            {
                ConstTypeCompatibility::Mismatch
            }
            _ => {
                let _ = expected;
                ConstTypeCompatibility::Unknown
            }
        }
    }

    fn resolved_const_cast_expected_compatibility(
        &mut self,
        inner: &ResolvedConstExpr,
        target: InternedTyId,
    ) -> ConstTypeCompatibility {
        let target = self.substitute_ty_generics(target);
        let Some(TyKind::Primitive(target)) = self.ty_kind(target) else {
            return ConstTypeCompatibility::Mismatch;
        };
        let Some(source) = self.resolved_const_expr_type(inner, None) else {
            return ConstTypeCompatibility::Unknown;
        };
        let ConstValueType::Runtime(source) = source else {
            return ConstTypeCompatibility::Mismatch;
        };
        let Some(TyKind::Primitive(source)) = self.ty_kind(source) else {
            return ConstTypeCompatibility::Mismatch;
        };
        let source_numeric = primitive_integer_layout(source, self.input.target.pointer_width)
            .is_some()
            || is_float_primitive(source);
        let target_numeric = primitive_integer_layout(target, self.input.target.pointer_width)
            .is_some()
            || is_float_primitive(target);
        if source_numeric && target_numeric {
            ConstTypeCompatibility::Unknown
        } else {
            ConstTypeCompatibility::Mismatch
        }
    }

    pub(super) fn probe_resolved_const_int_expr(
        &mut self,
        expr: &ResolvedConstExpr,
    ) -> Option<i128> {
        self.probe_resolved_const_eval(|this| {
            nia_const_eval::eval_resolved_const_int_expr(expr, this)
        })
        .and_then(IntConst::as_i128)
    }

    pub(super) fn probe_resolved_const_array_len_expr(
        &mut self,
        expr: &ResolvedConstExpr,
    ) -> Option<u64> {
        self.probe_resolved_const_eval(|this| {
            nia_const_eval::eval_resolved_const_array_len_expr(expr, this)
        })
    }

    fn probe_resolved_const_eval<T>(
        &mut self,
        evaluate: impl FnOnce(&mut Self) -> Result<T, nia_const_eval::ConstError>,
    ) -> Option<T> {
        let call_locals = self.call_locals.clone();
        let budget = self.const_eval_budget.clone();
        let diagnostic_len = self.diagnostics.len();
        let result = evaluate(self).ok();
        self.call_locals = call_locals;
        self.const_eval_budget = budget;
        self.diagnostics.truncate(diagnostic_len);
        result
    }

    pub(super) fn probe_type_generic_inference(
        &mut self,
        span: Span,
        expected: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut SymbolMap<InternedTyId>,
    ) -> Option<()> {
        self.infer_generics_from_tys(
            span,
            self.current_execution_module_id(),
            expected,
            actual,
            substitutions,
        )
        .ok()
    }

    pub(super) fn const_name_resolution_type(
        &mut self,
        resolution: ConstNameResolution,
    ) -> Option<ConstValueType> {
        match resolution {
            ConstNameResolution::Local(local_id) => self
                .call_local_type(local_id)
                .or_else(|| {
                    let ty = self
                        .typed_value_for_key(ConstKey::Local(local_id))
                        .map(|typed| typed.ty.clone())?;
                    self.value_type_for_module(ty, self.current_execution_module_id())
                })
                .or_else(|| {
                    self.explicit_type_for_key(ConstKey::Local(local_id))
                        .and_then(|ty| {
                            self.type_for_module_or_none(ty, self.current_execution_module_id())
                        })
                        .map(ConstValueType::Runtime)
                }),
            ConstNameResolution::Global(global_id) => self
                .typed_value_for_key(ConstKey::Global(global_id))
                .map(|typed| typed.ty.clone())
                .and_then(|ty| self.value_type_for_module(ty, self.current_execution_module_id()))
                .or_else(|| {
                    self.explicit_type_for_key(ConstKey::Global(global_id))
                        .and_then(|ty| {
                            self.type_for_module_or_none(ty, self.current_execution_module_id())
                        })
                        .map(ConstValueType::Runtime)
                }),
            ConstNameResolution::BuiltinAssociatedValue(value) => {
                let BuiltinAssociatedValue::PrimitiveIntLimit { primitive, .. } = value;
                Some(ConstValueType::Runtime(
                    self.current_runtime_primitive_type(primitive),
                ))
            }
            ConstNameResolution::AssociatedConstProjection(projection) => self
                .associated_const_projection_type(&projection)
                .map(ConstValueType::Runtime),
            ConstNameResolution::GenericParam(name) => self
                .active_execution_frames()
                .find_map(|frame| frame.const_substitutions.get(&name))
                .map(|arg| ConstValueType::Runtime(arg.ty)),
        }
    }

    pub(super) fn resolved_const_expr_type(
        &mut self,
        expr: &ResolvedConstExpr,
        expected: Option<InternedTyId>,
    ) -> Option<ConstValueType> {
        let inferred = self.resolved_const_expr_type_inner(expr, expected);
        if let Some(ConstValueType::Runtime(ty)) = inferred
            && let Some(types) = self.resolved_expr_types.last_mut()
        {
            types.insert(expr.span(), ty);
        }
        inferred
    }

    fn resolved_const_expr_type_inner(
        &mut self,
        expr: &ResolvedConstExpr,
        expected: Option<InternedTyId>,
    ) -> Option<ConstValueType> {
        match expr.kind() {
            ResolvedConstExprKind::Name(resolution) => self
                .resolved_const_enum_variant_value_type(expr)
                .or_else(|| self.const_name_resolution_type(resolution.clone())),
            ResolvedConstExprKind::Integer(text) => {
                if let Some(primitive) = integer_literal_suffix_ty(text) {
                    Some(ConstValueType::Runtime(
                        self.current_runtime_primitive_type(primitive),
                    ))
                } else {
                    expected
                        .filter(|ty| self.is_integer_runtime_type(*ty))
                        .map(ConstValueType::Runtime)
                }
            }
            ResolvedConstExprKind::Float(text) => {
                let primitive = float_literal_suffix_ty(text)
                    .or_else(|| {
                        expected.and_then(|ty| match self.ty_kind(ty) {
                            Some(TyKind::Primitive(primitive)) if primitive.is_float() => {
                                Some(primitive)
                            }
                            _ => None,
                        })
                    })
                    .unwrap_or(PrimitiveTy::F64);
                Some(ConstValueType::Runtime(
                    self.current_runtime_primitive_type(primitive),
                ))
            }
            ResolvedConstExprKind::Char(_) => Some(ConstValueType::Runtime(
                self.current_runtime_primitive_type(PrimitiveTy::Char),
            )),
            ResolvedConstExprKind::ByteChar(_) => Some(ConstValueType::Runtime(
                self.current_runtime_primitive_type(PrimitiveTy::U8),
            )),
            ResolvedConstExprKind::String(literal) => self.const_string_literal_type(literal),
            ResolvedConstExprKind::ByteString(literal) => self.const_byte_string_literal_type(
                nia_const_eval::eval_byte_string_literal(literal)?.len() as u64,
            ),
            ResolvedConstExprKind::Embed { path } => {
                let path = nia_const_eval::eval_string_literal(path)?;
                let resolved = super::env_impl::resolve_embed_path(
                    self.current_execution_source_path()?.as_str(),
                    &path,
                );
                let len = std::fs::metadata(resolved).ok()?.len();
                self.const_byte_string_literal_type(len)
            }
            ResolvedConstExprKind::Bool(_) => Some(ConstValueType::Runtime(
                self.current_runtime_primitive_type(PrimitiveTy::Bool),
            )),
            ResolvedConstExprKind::Tuple(elems) => {
                let expected_elems = expected.and_then(|expected| match self.ty_kind(expected) {
                    Some(TyKind::Tuple(expected_elems)) if expected_elems.len() == elems.len() => {
                        Some(expected_elems)
                    }
                    _ => None,
                });
                let elem_types = elems
                    .iter()
                    .enumerate()
                    .map(|(index, elem)| {
                        self.resolved_const_arg_runtime_type(
                            elem,
                            expected_elems.as_ref().map(|elems| elems[index]),
                        )
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(ConstValueType::Runtime(
                    self.current_runtime_tuple_type(elem_types),
                ))
            }
            ResolvedConstExprKind::TupleField { lhs, index } => {
                let lhs_ty = self.resolved_const_arg_runtime_type(lhs, None)?;
                let Some(TyKind::Tuple(elems)) = self.ty_kind(lhs_ty) else {
                    return None;
                };
                Some(ConstValueType::Runtime(*elems.get(*index)?))
            }
            ResolvedConstExprKind::StructLiteral { ty, fields } => {
                self.resolved_const_aggregate_literal_type(expr.span(), fields, *ty)
            }
            ResolvedConstExprKind::TupleStructLiteral {
                def_id,
                generic_args,
                fields,
            } => self
                .resolved_const_tuple_struct_literal_type(
                    expr.span(),
                    *def_id,
                    generic_args,
                    fields,
                )
                .map(ConstValueType::Runtime),
            ResolvedConstExprKind::ArrayLiteral { elems } => {
                self.resolved_const_array_literal_type(expr.span(), elems, expected)
            }
            ResolvedConstExprKind::EnumStructLiteral { variant, fields } => self
                .resolved_const_named_enum_literal_type(expr.span(), variant, fields)
                .map(ConstValueType::Runtime),
            ResolvedConstExprKind::OptionalSome { expr: inner } => {
                let expected_elem = expected.and_then(|expected| match self.ty_kind(expected) {
                    Some(TyKind::Optional { elem }) => Some(elem),
                    _ => None,
                });
                let elem = self.resolved_const_arg_runtime_type(inner, expected_elem)?;
                self.const_runtime_type(
                    elem,
                    |elem| TyKind::Optional { elem },
                    self.current_execution_module_id(),
                )
                .map(ConstValueType::Runtime)
            }
            ResolvedConstExprKind::ErrorOk { expr: inner } => {
                let (error, value) = self.expected_error_union_parts(expected?)?;
                let actual_value = self.resolved_const_arg_runtime_type(inner, Some(value))?;
                self.const_error_union_type(error, actual_value)
                    .map(ConstValueType::Runtime)
            }
            ResolvedConstExprKind::ErrorErr { expr: inner } => {
                let (error, value) = self.expected_error_union_parts(expected?)?;
                let actual_error = self.resolved_const_arg_runtime_type(inner, Some(error))?;
                self.const_error_union_type(actual_error, value)
                    .map(ConstValueType::Runtime)
            }
            ResolvedConstExprKind::Try { expr: inner } => {
                let inner_ty = self.resolved_const_arg_runtime_type(inner, None)?;
                self.prepare_resolved_const_try_error_conversion(expr.span(), inner_ty);
                let payload = match self.ty_kind(inner_ty)? {
                    TyKind::Optional { elem } => elem,
                    TyKind::ErrorUnion { value, .. } => value,
                    _ => return None,
                };
                self.type_for_module_or_none(payload, self.current_execution_module_id())
                    .map(ConstValueType::Runtime)
            }
            ResolvedConstExprKind::Field { lhs, name } => {
                let lhs_ty = self.resolved_const_expr_type(lhs, None)?;
                self.const_field_type(lhs_ty, name)
            }
            ResolvedConstExprKind::Cast { expr: inner, ty } => {
                self.resolved_const_cast_type(inner, *ty)
            }
            ResolvedConstExprKind::Index { lhs, index } => {
                let lhs_ty = self.resolved_const_expr_type(lhs, None);
                match lhs_ty {
                    Some(lhs_ty) => self.resolved_const_index_type(expr.span(), lhs_ty, index),
                    None => {
                        self.visit_resolved_const_index_operand(index);
                        None
                    }
                }
            }
            ResolvedConstExprKind::Slice { lhs, range } => {
                let lhs_ty = self.resolved_const_expr_type(lhs, None);
                match lhs_ty {
                    Some(lhs_ty) => {
                        self.resolved_const_slice_type(expr.span(), lhs_ty, range, expected)
                    }
                    None => {
                        self.visit_resolved_const_slice_bounds(range);
                        None
                    }
                }
            }
            ResolvedConstExprKind::Range(range) => self.resolved_const_range_type(range, expected),
            ResolvedConstExprKind::Binary { lhs, op, rhs } => {
                self.resolved_const_binary_expr_type(lhs, *op, rhs, expected)
            }
            ResolvedConstExprKind::Unary { op, expr: inner } => {
                self.resolved_const_unary_expr_type(*op, inner, expected)
            }
            ResolvedConstExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.resolved_const_if_expr_type(
                cond,
                then_branch,
                else_branch.as_deref(),
                expected,
            ),
            ResolvedConstExprKind::Match(matched) => {
                self.resolved_const_match_expr_type(matched, expected)
            }
            ResolvedConstExprKind::Block(block) => {
                self.resolved_const_block_tail_type(block, expected)
            }
            ResolvedConstExprKind::BuiltinConstValue(_) => expected.map(ConstValueType::Runtime),
            ResolvedConstExprKind::BuiltinValue(ValueBuiltin::Error) => None,
            ResolvedConstExprKind::CompileError { message } => {
                let _ = self.resolved_const_expr_type(message, None);
                expected.map(ConstValueType::Runtime)
            }
            ResolvedConstExprKind::Trap => expected.map(ConstValueType::Runtime),
            ResolvedConstExprKind::Call {
                callee,
                generic_args,
                args,
            } => {
                if self.resolved_const_enum_variant(callee).is_some() {
                    self.resolved_const_tuple_enum_literal_type(expr.span(), callee, args)
                        .map(ConstValueType::Runtime)
                } else {
                    self.resolved_const_call_return_type(
                        expr.span(),
                        callee,
                        generic_args,
                        args,
                        expected,
                    )
                    .map(ConstValueType::Runtime)
                }
            }
            ResolvedConstExprKind::LayoutBuiltin { .. }
            | ResolvedConstExprKind::FieldOffsetBuiltin { .. }
            | ResolvedConstExprKind::Method { .. }
            | ResolvedConstExprKind::AssociatedFunction { .. }
            | ResolvedConstExprKind::Null => None,
            ResolvedConstExprKind::Assign(assign) => {
                self.check_resolved_const_assignment(expr.span(), assign);
                Some(ConstValueType::Runtime(
                    self.current_runtime_tuple_type(Vec::new()),
                ))
            }
        }
    }

    pub(super) fn prepare_resolved_const_try_error_conversion(
        &mut self,
        span: Span,
        inner_ty: InternedTyId,
    ) {
        if self
            .call_locals
            .last()
            .is_some_and(|frame| frame.checked_try_error_conversions.contains(&span))
        {
            return;
        }
        if let Some(frame) = self.call_locals.last_mut() {
            frame.checked_try_error_conversions.insert(span);
            frame.try_error_conversions.remove(&span);
        }

        let Some(TyKind::ErrorUnion {
            error: source_error,
            ..
        }) = self.ty_kind(inner_ty)
        else {
            return;
        };
        let return_ty = self
            .active_execution_frames()
            .find_map(|frame| frame.return_type);
        let Some(return_ty) = return_ty else {
            return;
        };
        let return_ty = self.substitute_ty_generics(return_ty);
        let Some((target_error, _)) = self.expected_error_union_parts(return_ty) else {
            return;
        };
        if self.const_function_types_match(source_error, target_error) {
            return;
        }

        let callee = match self.resolved_const_into_error_method(source_error, target_error) {
            ResolvedConstCalleeSelection::Unique(callee) => callee,
            ResolvedConstCalleeSelection::NoMatch => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::CONST,
                    span,
                    "automatic `IntoError` conversion during const evaluation requires a unique concrete trait witness",
                ));
                return;
            }
            ResolvedConstCalleeSelection::Ambiguous => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::CONST,
                    span,
                    "ambiguous automatic `IntoError` conversion during const evaluation",
                ));
                return;
            }
        };
        let is_const = self
            .function_signatures_for_module(callee.function_id.module_id)
            .and_then(|signatures| {
                signatures
                    .as_ref()
                    .functions
                    .get(&callee.function_id.def_id)
                    .map(|signature| signature.is_const)
            })
            .unwrap_or(false);
        if !is_const {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::CONST,
                span,
                "automatic `IntoError` conversion during const evaluation requires `into_error` to be declared `const fn`",
            ));
            return;
        }
        if let Some(frame) = self.call_locals.last_mut() {
            frame.try_error_conversions.insert(span, callee);
        }
    }

    pub(super) fn check_resolved_const_assignment(
        &mut self,
        span: Span,
        assign: &ResolvedConstAssign,
    ) {
        let ResolvedConstAssignTargetKind::Local {
            name,
            local_id,
            path,
            ..
        } = assign.lhs().kind();
        let Some(mut target_ty) = self.call_local_type(*local_id) else {
            let _ = self.resolved_const_expr_type(assign.rhs(), None);
            return;
        };

        if self.const_local_is_mutable(*local_id) == Some(false) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!(
                    "cannot assign to immutable const local `{}`",
                    self.symbol_name(*name)
                ),
            ));
        }

        for (element_index, element) in path.iter().enumerate() {
            if self.const_assignment_target_type_is_unresolved(&target_ty) {
                for remaining in &path[element_index..] {
                    if let ResolvedConstAssignPathElemKind::Index { index, .. } = remaining.kind() {
                        let _ = self.resolved_const_expr_type(index, None);
                    }
                }
                let _ = self.resolved_const_expr_type(assign.rhs(), None);
                return;
            }
            target_ty = match element.kind() {
                ResolvedConstAssignPathElemKind::Field {
                    span: field_span,
                    name,
                } => match self.const_field_type(target_ty, name) {
                    Some(field_ty) => field_ty,
                    None => {
                        self.push_invalid_const_assignment_target(*field_span);
                        let _ = self.resolved_const_expr_type(assign.rhs(), None);
                        return;
                    }
                },
                ResolvedConstAssignPathElemKind::Index {
                    span: index_span,
                    index,
                } => {
                    let _ = self.resolved_const_expr_type(index, None);
                    match self.const_assignment_index_elem_type(&target_ty) {
                        Some(elem_ty) => elem_ty,
                        None => {
                            self.push_invalid_const_assignment_target(*index_span);
                            let _ = self.resolved_const_expr_type(assign.rhs(), None);
                            return;
                        }
                    }
                }
            };
        }

        let expected = target_ty.runtime();
        let actual = self.resolved_const_expr_type(assign.rhs(), expected);
        if !matches!(assign.op(), ConstAssignOp::Assign)
            && !self.const_compound_assignment_type_is_supported(assign.op(), &target_ty)
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "const compound assignment requires compatible numeric operands".to_string(),
            ));
        }
        if let Some(actual) = actual
            && !self.const_assignment_types_match(&target_ty, &actual)
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                assign.rhs().span(),
                "const assignment value does not match the target type".to_string(),
            ));
        }
    }

    fn const_assignment_target_type_is_unresolved(&self, target: &ConstValueType) -> bool {
        let ConstValueType::Runtime(target) = target else {
            return false;
        };
        matches!(
            self.ty_kind(*target),
            Some(
                TyKind::GenericParam(_)
                    | TyKind::SelfParam
                    | TyKind::Projection { .. }
                    | TyKind::Error
            )
        )
    }

    fn const_assignment_index_elem_type(
        &mut self,
        target: &ConstValueType,
    ) -> Option<ConstValueType> {
        match target {
            ConstValueType::Array { elem, .. } => Some((**elem).clone()),
            ConstValueType::Runtime(ty) => match self.ty_kind(*ty)? {
                TyKind::Array { elem, .. }
                | TyKind::Slice {
                    is_readonly: false,
                    elem,
                } => self
                    .type_for_module_or_none(elem, self.current_execution_module_id())
                    .map(ConstValueType::Runtime),
                _ => None,
            },
            ConstValueType::Int | ConstValueType::Bool | ConstValueType::String => None,
        }
    }

    fn const_compound_assignment_type_is_supported(
        &self,
        op: ConstAssignOp,
        target: &ConstValueType,
    ) -> bool {
        let ConstValueType::Runtime(target) = target else {
            return false;
        };
        if matches!(self.ty_kind(*target), Some(TyKind::GenericParam(_))) {
            return true;
        }
        match op {
            ConstAssignOp::Assign => true,
            ConstAssignOp::Shl
            | ConstAssignOp::Shr
            | ConstAssignOp::BitAnd
            | ConstAssignOp::BitXor
            | ConstAssignOp::BitOr => self.is_integer_runtime_type(*target),
            ConstAssignOp::Add
            | ConstAssignOp::Sub
            | ConstAssignOp::Mul
            | ConstAssignOp::Div
            | ConstAssignOp::Rem => {
                self.is_integer_runtime_type(*target) || self.is_float_runtime_type(*target)
            }
        }
    }

    fn const_assignment_types_match(
        &mut self,
        expected: &ConstValueType,
        actual: &ConstValueType,
    ) -> bool {
        match (expected, actual) {
            (ConstValueType::Runtime(expected), ConstValueType::Runtime(actual)) => {
                self.const_function_types_match(*expected, *actual)
            }
            _ => expected == actual,
        }
    }

    fn push_invalid_const_assignment_target(&mut self, span: Span) {
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            span,
            "invalid const assignment target path".to_string(),
        ));
    }

    pub(super) fn find_resolved_pattern_local_type(
        &mut self,
        matched: &ResolvedConstMatch,
        local_id: LocalId,
    ) -> Option<InternedTyId> {
        let target_ty = self.resolved_const_arg_runtime_type(matched.target(), None)?;
        for arm in matched.arms() {
            for pattern in arm.patterns() {
                if let Some(ty) = self.resolved_pattern_binding_type(pattern, target_ty, local_id) {
                    return Some(ty);
                }
            }
        }
        None
    }

    fn resolved_const_enum_variant_value_type(
        &self,
        variant: &ResolvedConstExpr,
    ) -> Option<ConstValueType> {
        let (enum_id, variant) = self.resolved_const_enum_variant(variant)?;
        matches!(
            variant.payload,
            nia_item_signatures::EnumVariantPayloadSignature::Unit
        )
        .then(|| ConstValueType::Runtime(self.enum_ty_in_current_module(enum_id)))
    }

    fn resolved_const_tuple_enum_literal_type(
        &mut self,
        span: Span,
        callee: &ResolvedConstExpr,
        args: &[ResolvedConstExpr],
    ) -> Option<InternedTyId> {
        let (enum_id, variant) = self.resolved_const_enum_variant(callee)?;
        let nia_item_signatures::EnumVariantPayloadSignature::Tuple(field_tys) = variant.payload
        else {
            for arg in args {
                let _ = self.resolved_const_expr_type(arg, None);
            }
            self.push_const_type_error(span, "const enum variant does not have a tuple payload");
            return None;
        };
        let arity_matches = field_tys.len() == args.len();
        if !arity_matches {
            self.push_const_type_error(
                span,
                &format!(
                    "const enum tuple payload length mismatch: expected {}, got {}",
                    field_tys.len(),
                    args.len()
                ),
            );
        }
        let current_module = self.current_execution_module_id();
        let mut types_match = true;
        for (index, arg) in args.iter().enumerate() {
            let Some(field_ty) = field_tys.get(index).copied() else {
                let _ = self.resolved_const_expr_type(arg, None);
                continue;
            };
            let Some(field_ty) = self.type_for_module_or_none(field_ty, current_module) else {
                let _ = self.resolved_const_expr_type(arg, None);
                continue;
            };
            let actual = self.resolved_const_contextual_value_type(arg, field_ty);
            let Some(actual) = actual else {
                continue;
            };
            if !self.const_function_types_match(field_ty, actual) {
                if self.const_runtime_type_is_known(field_ty)
                    && self.const_runtime_type_is_known(actual)
                {
                    self.push_const_type_error(
                        arg.span(),
                        "const enum payload value does not match its expected type",
                    );
                }
                types_match = false;
            }
        }
        (arity_matches && types_match).then(|| self.enum_ty_in_current_module(enum_id))
    }

    fn resolved_const_named_enum_literal_type(
        &mut self,
        span: Span,
        target: &ResolvedConstExpr,
        fields: &[ResolvedConstFieldInit],
    ) -> Option<InternedTyId> {
        let (enum_id, variant) = self.resolved_const_enum_variant(target)?;
        let nia_item_signatures::EnumVariantPayloadSignature::Named(expected) = variant.payload
        else {
            for field in fields {
                let _ = self.resolved_const_expr_type(field.value(), None);
            }
            self.push_const_type_error(span, "const enum variant does not have a named payload");
            return None;
        };
        let field_set = check_required_field_set(
            fields
                .iter()
                .map(|field| NamedField::new(field.span(), *field.name_symbol())),
            expected.iter().map(|field| field.name),
        );
        let fields_are_valid = field_set.is_valid();
        for field in &field_set.duplicate_fields {
            let name = self.symbol_name(field.name);
            self.push_const_type_error(field.span, &format!("duplicate const enum field `{name}`"));
        }
        for field in &field_set.unknown_fields {
            let name = self.symbol_name(field.name);
            self.push_const_type_error(field.span, &format!("unknown const enum field `{name}`"));
        }
        for name in &field_set.missing_fields {
            let name = self.symbol_name(*name);
            self.push_const_type_error(span, &format!("missing const enum field `{name}`"));
        }
        let current_module = self.current_execution_module_id();
        let mut types_match = true;
        for field in fields {
            let Some(field_ty) = expected
                .iter()
                .find(|expected| expected.name == *field.name_symbol())
                .map(|field| field.ty)
            else {
                let _ = self.resolved_const_expr_type(field.value(), None);
                continue;
            };
            let Some(field_ty) = self.type_for_module_or_none(field_ty, current_module) else {
                let _ = self.resolved_const_expr_type(field.value(), None);
                continue;
            };
            let actual = self.resolved_const_contextual_value_type(field.value(), field_ty);
            let Some(actual) = actual else {
                continue;
            };
            if !self.const_function_types_match(field_ty, actual) {
                if self.const_runtime_type_is_known(field_ty)
                    && self.const_runtime_type_is_known(actual)
                {
                    self.push_const_type_error(
                        field.value().span(),
                        "const enum payload value does not match its expected type",
                    );
                }
                types_match = false;
            }
        }
        (fields_are_valid && types_match).then(|| self.enum_ty_in_current_module(enum_id))
    }

    fn resolved_const_tuple_struct_literal_type(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        generic_args: &[ResolvedConstGenericArg],
        fields: &[ResolvedConstFieldInit],
    ) -> Option<InternedTyId> {
        let signature = self.struct_signature_for(def_id)?;
        if !signature.is_tuple {
            self.push_const_type_error(span, "const tuple constructor targets a named struct");
            return None;
        }
        if signature.fields.len() != fields.len() {
            self.push_const_type_error(
                span,
                &format!(
                    "const tuple struct expects {} values, found {}",
                    signature.fields.len(),
                    fields.len()
                ),
            );
        }
        let (args, const_args) = self.resolve_const_aggregate_generic_args(
            span,
            def_id.module_id,
            &signature.generic_params,
            generic_args,
        )?;
        let field_tys = self.const_struct_field_types(&signature, &args, &const_args)?;
        let current_module = self.current_execution_module_id();
        let mut valid = signature.fields.len() == fields.len();
        for (field, expected) in fields.iter().zip(signature.fields.iter()) {
            let Some(expected_ty) = field_tys.get(&expected.name).copied() else {
                let _ = self.resolved_const_expr_type(field.value(), None);
                valid = false;
                continue;
            };
            let Some(actual_ty) =
                self.resolved_const_contextual_value_type(field.value(), expected_ty)
            else {
                valid = false;
                continue;
            };
            valid &= self.const_function_types_match(expected_ty, actual_ty);
        }
        valid.then(|| {
            self.type_contexts
                .get(&current_module)
                .expect("current execution module must have a type context")
                .intern(TyKind::Nominal {
                    def_id,
                    args,
                    const_args,
                })
        })
    }

    fn resolve_const_aggregate_generic_args(
        &mut self,
        span: Span,
        signature_module_id: ModuleId,
        generic_params: &[nia_item_signatures::GenericParamSignature],
        generic_args: &[ResolvedConstGenericArg],
    ) -> Option<(Vec<InternedTyId>, Vec<ConstGenericArg>)> {
        // Generic parameters are declared in one source-order list but the
        // nominal type stores type and const arguments in separate vectors.
        // Validate the paired kinds here, then split them while evaluating
        // const arguments in the defining module's type context. This keeps
        // tuple-struct construction aligned with named aggregate literals and
        // prevents a generic field from being checked against its raw symbol.
        if let ArityCheck::Mismatch { actual, .. } =
            check_exact_arity(generic_params.len(), generic_args.len())
        {
            self.push_const_type_error(
                span,
                &format!(
                    "tuple struct generic argument count mismatch: expected {}, got {}",
                    generic_params.len(),
                    actual
                ),
            );
            return None;
        }
        let mut args = Vec::new();
        let mut const_args = Vec::new();
        for (param, arg) in generic_params.iter().zip(generic_args) {
            match (&param.kind, arg) {
                (GenericParamSignatureKind::Type, ResolvedConstGenericArg::Type(arg)) => {
                    args.push(self.type_for_module_or_none(arg.ty(), signature_module_id)?);
                }
                (GenericParamSignatureKind::Const { ty }, ResolvedConstGenericArg::Const(expr)) => {
                    let ty = self.type_for_module_or_none(*ty, signature_module_id)?;
                    let value = match self.const_generic_arg_from_resolved_expr(
                        expr,
                        ty,
                        signature_module_id,
                    ) {
                        Ok(value) => value,
                        Err(error) => {
                            self.push_const_type_error(error.span, &error.message);
                            return None;
                        }
                    };
                    const_args.push(ConstGenericArg { ty, value });
                }
                (GenericParamSignatureKind::Type, _) => {
                    self.push_const_type_error(
                        span,
                        "tuple struct generic argument must be a type",
                    );
                    return None;
                }
                (GenericParamSignatureKind::Const { .. }, _) => {
                    self.push_const_type_error(
                        span,
                        "tuple struct generic argument must be a const value",
                    );
                    return None;
                }
            }
        }
        Some((args, const_args))
    }

    pub(super) fn resolved_const_enum_pattern_fields<'a>(
        &mut self,
        variant_expr: &ResolvedConstExpr,
        fields: &'a ConstEnumPatternFields<ResolvedConstPattern>,
        target_ty: InternedTyId,
    ) -> Option<Vec<(&'a ResolvedConstPattern, InternedTyId)>> {
        let (enum_id, variant) = self.resolved_const_enum_variant(variant_expr)?;
        let (target_enum, target_args, target_const_args) =
            self.expected_nominal_parts(target_ty)?;
        if target_enum != enum_id || !target_args.is_empty() || !target_const_args.is_empty() {
            return None;
        }
        let current_module = self.current_execution_module_id();
        match (fields, variant.payload) {
            (
                ConstEnumPatternFields::Tuple(patterns),
                nia_item_signatures::EnumVariantPayloadSignature::Tuple(expected),
            ) => {
                if patterns.len() != expected.len() {
                    return None;
                }
                patterns
                    .iter()
                    .zip(expected)
                    .map(|(pattern, ty)| {
                        Some((pattern, self.type_for_module_or_none(ty, current_module)?))
                    })
                    .collect()
            }
            (
                ConstEnumPatternFields::Named {
                    fields: patterns,
                    rest,
                },
                nia_item_signatures::EnumVariantPayloadSignature::Named(expected),
            ) => {
                let field_set = check_required_field_set(
                    patterns
                        .iter()
                        .map(|field| NamedField::new(field.span, field.name)),
                    expected.iter().map(|field| field.name),
                );
                if !field_set.duplicate_fields.is_empty()
                    || !field_set.unknown_fields.is_empty()
                    || (rest.is_none() && !field_set.missing_fields.is_empty())
                {
                    return None;
                }
                patterns
                    .iter()
                    .map(|field| {
                        let ty = expected
                            .iter()
                            .find(|expected| expected.name == field.name)?
                            .ty;
                        Some((
                            &field.pattern,
                            self.type_for_module_or_none(ty, current_module)?,
                        ))
                    })
                    .collect()
            }
            _ => None,
        }
    }

    pub(super) fn resolved_const_struct_pattern_fields<'a>(
        &mut self,
        def_id: GlobalDefId,
        fields: &'a [ConstNamedPatternField<ResolvedConstPattern>],
        rest: Option<nia_span::Span>,
        target_ty: InternedTyId,
    ) -> Option<Vec<(&'a ResolvedConstPattern, InternedTyId)>> {
        let (target_def, _, _) = self.expected_nominal_parts(target_ty)?;
        if target_def != def_id || self.def_kind_of(def_id) != Some(DefKind::Struct) {
            return None;
        }
        let expected = self.struct_signature_for(def_id)?.fields;
        let field_set = check_required_field_set(
            fields
                .iter()
                .map(|field| NamedField::new(field.span, field.name)),
            expected.iter().map(|field| field.name),
        );
        if !field_set.duplicate_fields.is_empty()
            || !field_set.unknown_fields.is_empty()
            || (rest.is_none() && !field_set.missing_fields.is_empty())
        {
            return None;
        }

        // Resolve against the instantiated target, not the constructor spelling: patterns omit
        // generic arguments and inherit them from the value being destructured.
        let mut resolved = Vec::with_capacity(fields.len());
        for expected in &expected {
            let Some(field) = fields.iter().find(|field| field.name == expected.name) else {
                continue;
            };
            let ty = self.const_nominal_aggregate_field_type(target_ty, &expected.name)?;
            resolved.push((&field.pattern, ty));
        }
        Some(resolved)
    }

    pub(super) fn resolved_pattern_binding_type(
        &mut self,
        pattern: &ResolvedConstPattern,
        target_ty: InternedTyId,
        local_id: LocalId,
    ) -> Option<InternedTyId> {
        match pattern.kind() {
            ResolvedConstPatternKind::Bind {
                local_id: pattern_local,
                ..
            } => (*pattern_local == local_id).then_some(target_ty),
            ResolvedConstPatternKind::Pointer { pattern, .. }
            | ResolvedConstPatternKind::MutPointer { pattern, .. } => {
                let TyKind::Pointer { elem, .. } = self.ty_kind(target_ty)? else {
                    return None;
                };
                self.resolved_pattern_binding_type(pattern, elem, local_id)
            }
            ResolvedConstPatternKind::OptionalSome { pattern, .. } => {
                let TyKind::Optional { elem } = self.ty_kind(target_ty)? else {
                    return None;
                };
                self.resolved_pattern_binding_type(pattern, elem, local_id)
            }
            ResolvedConstPatternKind::ErrorOk { pattern, .. } => {
                let TyKind::ErrorUnion { value, .. } = self.ty_kind(target_ty)? else {
                    return None;
                };
                self.resolved_pattern_binding_type(pattern, value, local_id)
            }
            ResolvedConstPatternKind::ErrorErr { pattern, .. } => {
                let TyKind::ErrorUnion { error, .. } = self.ty_kind(target_ty)? else {
                    return None;
                };
                self.resolved_pattern_binding_type(pattern, error, local_id)
            }
            ResolvedConstPatternKind::Tuple { patterns, .. } => {
                let TyKind::Tuple(elems) = self.ty_kind(target_ty)? else {
                    return None;
                };
                (patterns.len() == elems.len()).then_some(())?;
                patterns.iter().zip(elems).find_map(|(pattern, elem)| {
                    self.resolved_pattern_binding_type(pattern, elem, local_id)
                })
            }
            ResolvedConstPatternKind::EnumVariant {
                variant, fields, ..
            } => self
                .resolved_const_enum_pattern_fields(variant, fields, target_ty)?
                .into_iter()
                .find_map(|(pattern, ty)| {
                    self.resolved_pattern_binding_type(pattern, ty, local_id)
                }),
            ResolvedConstPatternKind::Struct {
                def_id,
                fields,
                rest,
                ..
            } => self
                .resolved_const_struct_pattern_fields(*def_id, fields, *rest, target_ty)?
                .into_iter()
                .find_map(|(pattern, ty)| {
                    self.resolved_pattern_binding_type(pattern, ty, local_id)
                }),
            ResolvedConstPatternKind::Wildcard { .. }
            | ResolvedConstPatternKind::OptionalNull { .. }
            | ResolvedConstPatternKind::Expr(_)
            | ResolvedConstPatternKind::Range { .. } => None,
        }
    }

    pub(super) fn const_field_type(
        &mut self,
        lhs: ConstValueType,
        name: &SymbolId,
    ) -> Option<ConstValueType> {
        match &lhs {
            ConstValueType::Runtime(ty) => self
                .const_nominal_aggregate_field_type(*ty, name)
                .map(ConstValueType::Runtime),
            ConstValueType::Array { .. }
            | ConstValueType::Int
            | ConstValueType::Bool
            | ConstValueType::String => None,
        }
    }

    pub(super) fn array_len_const_value(&mut self, len: ArrayLenTy) -> Option<u64> {
        match len {
            ArrayLenTy::ConstValue(len) => Some(len),
            ArrayLenTy::ConstExpr(id) => self
                .array_lengths
                .get(&id)
                .copied()
                .or_else(|| self.eval_array_len_const_expr_id(id)),
            ArrayLenTy::Builtin { builtin, ty } => {
                let ConstValue::Int(value) = self
                    .resolve_layout_builtin_for_ty(Span::default(), builtin, ty)
                    .ok()?
                else {
                    return None;
                };
                u64::try_from(value.bits()).ok()
            }
            ArrayLenTy::Infer | ArrayLenTy::GenericParam(_) => None,
        }
    }

    pub(super) fn eval_array_len_const_expr_id(&mut self, id: GlobalConstExprId) -> Option<u64> {
        let expr = if id.module_id == self.input.defs.module_id {
            self.input.module.const_exprs().get(&id)?.clone()
        } else {
            (self.input.program.module?)(id.module_id)?
                .const_exprs()
                .get(&id)?
                .clone()
        };
        let value = self.with_execution_module(id.module_id, |this| {
            this.eval_resolved_array_len_expr(&expr)
        })?;
        self.array_lengths.insert(id, value);
        Some(value)
    }

    pub(super) fn value_type_for_module(
        &mut self,
        ty: ConstValueType,
        target_module_id: ModuleId,
    ) -> Option<ConstValueType> {
        match ty {
            ConstValueType::Runtime(ty) => self
                .type_for_module_or_none(ty, target_module_id)
                .map(ConstValueType::Runtime),
            ConstValueType::Array { elem, len } => Some(ConstValueType::Array {
                elem: Box::new(self.value_type_for_module(*elem, target_module_id)?),
                len,
            }),
            ConstValueType::Int => Some(ConstValueType::Int),
            ConstValueType::Bool => Some(ConstValueType::Bool),
            ConstValueType::String => Some(ConstValueType::String),
        }
    }

    pub(super) fn type_contains_generic(&self, ty: InternedTyId) -> bool {
        let mut seen = HashSet::new();
        self.type_contains_generic_inner(ty, &mut seen)
    }

    pub(super) fn type_contains_generic_inner(
        &self,
        ty: InternedTyId,
        seen: &mut HashSet<InternedTyId>,
    ) -> bool {
        if !seen.insert(ty) {
            return false;
        }
        match self.ty_kind(ty) {
            Some(TyKind::GenericParam(_) | TyKind::SelfParam) => true,
            Some(TyKind::Tuple(elems)) => elems
                .into_iter()
                .any(|elem| self.type_contains_generic_inner(elem, seen)),
            Some(TyKind::ClosureState {
                captures,
                params,
                return_type,
                ..
            }) => {
                captures
                    .into_iter()
                    .chain(params)
                    .any(|ty| self.type_contains_generic_inner(ty, seen))
                    || self.type_contains_generic_inner(return_type, seen)
            }
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::VolatilePointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::SlicePointee { elem })
            | Some(TyKind::Array { elem, .. })
            | Some(TyKind::Optional { elem }) => self.type_contains_generic_inner(elem, seen),
            Some(TyKind::Range { bound, .. }) => {
                bound.is_some_and(|bound| self.type_contains_generic_inner(bound, seen))
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            })
            | Some(TyKind::Callable {
                params,
                return_type,
                ..
            })
            | Some(TyKind::CallablePointee {
                params,
                return_type,
            }) => {
                params
                    .into_iter()
                    .any(|param| self.type_contains_generic_inner(param, seen))
                    || self.type_contains_generic_inner(return_type, seen)
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                self.type_contains_generic_inner(error, seen)
                    || self.type_contains_generic_inner(value, seen)
            }
            Some(TyKind::Nominal { args, .. }) | Some(TyKind::BuiltinTrait { args, .. }) => args
                .into_iter()
                .any(|arg| self.type_contains_generic_inner(arg, seen)),
            Some(TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_args,
                associated_type_bindings,
                ..
            }) => {
                trait_args
                    .into_iter()
                    .any(|arg| self.type_contains_generic_inner(arg, seen))
                    || associated_type_bindings
                        .into_iter()
                        .any(|binding| self.type_contains_generic_inner(binding.ty, seen))
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.type_contains_generic_inner(self_ty, seen)
                    || trait_args
                        .into_iter()
                        .any(|arg| self.type_contains_generic_inner(arg, seen))
            }
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::Opaque
                | TyKind::Primitive(_)
                | TyKind::BuiltinType(_)
                | TyKind::Vector { .. },
            )
            | None => false,
        }
    }

    pub(super) fn resolved_const_match_expr_type(
        &mut self,
        matched: &ResolvedConstMatch,
        expected: Option<InternedTyId>,
    ) -> Option<ConstValueType> {
        let target_ty = self.resolved_const_arg_runtime_type(matched.target(), None);
        let expected = expected.and_then(|expected| self.usable_const_expected_type(expected));
        let mut result_ty = expected.map(ConstValueType::Runtime);
        let mut saw_value_arm = false;
        let mut all_arms_typed = true;
        for arm in matched.arms() {
            if target_ty.is_some_and(|target_ty| {
                self.const_runtime_type_is_known(target_ty)
                    && self
                        .resolved_const_patterns_have_definite_mismatch(arm.patterns(), target_ty)
            }) {
                self.push_const_type_error(
                    arm.span(),
                    "const match pattern does not match the target type",
                );
                let _ = self.resolved_const_match_arm_body_type(
                    arm.body(),
                    result_ty.as_ref().and_then(ConstValueType::runtime),
                );
                all_arms_typed = false;
                continue;
            }
            let diagnostics_before_arm = self.diagnostics.len();
            let arm_ty = result_ty
                .clone()
                .and_then(|expected| {
                    let runtime_expected = expected.runtime();
                    let arm_ty =
                        self.resolved_const_match_arm_type(arm, target_ty, runtime_expected)?;
                    (arm_ty == ConstArmType::Value(expected)).then_some(arm_ty)
                })
                .or_else(|| {
                    self.resolved_const_match_arm_type(
                        arm,
                        target_ty,
                        result_ty.as_ref()?.runtime(),
                    )
                })
                .or_else(|| self.resolved_const_match_arm_type(arm, target_ty, None));
            let Some(arm_ty) = arm_ty else {
                if self.diagnostics.len() == diagnostics_before_arm {
                    let _ = self.resolved_const_match_arm_body_type(
                        arm.body(),
                        result_ty.as_ref().and_then(ConstValueType::runtime),
                    );
                }
                all_arms_typed = false;
                continue;
            };
            let ConstArmType::Value(arm_ty) = arm_ty else {
                continue;
            };
            saw_value_arm = true;
            match &result_ty {
                Some(result_ty) if *result_ty != arm_ty => {
                    if self.const_value_types_have_known_mismatch(result_ty, &arm_ty) {
                        self.push_const_type_error(
                            arm.span(),
                            "const match arms have incompatible result types",
                        );
                    }
                    all_arms_typed = false;
                }
                Some(_) => {}
                None => result_ty = Some(arm_ty),
            }
        }
        if !all_arms_typed {
            return None;
        }
        if let Some(target_ty) = target_ty {
            self.check_resolved_const_match_coverage(matched, target_ty);
        }
        saw_value_arm.then_some(result_ty).flatten()
    }

    pub(super) fn const_value_types_have_known_mismatch(
        &self,
        lhs: &ConstValueType,
        rhs: &ConstValueType,
    ) -> bool {
        match (lhs, rhs) {
            (ConstValueType::Runtime(lhs), ConstValueType::Runtime(rhs)) => {
                self.const_runtime_type_is_known(*lhs) && self.const_runtime_type_is_known(*rhs)
            }
            _ => true,
        }
    }

    pub(super) fn resolved_const_match_arm_type(
        &mut self,
        arm: &nia_const_ir::ResolvedConstMatchArm,
        target_ty: Option<InternedTyId>,
        expected: Option<InternedTyId>,
    ) -> Option<ConstArmType> {
        let target_ty = target_ty?;
        self.check_resolved_const_patterns(arm.patterns(), target_ty)?;
        if !self.resolved_const_match_arm_binds_pattern_locals(arm) {
            return self.resolved_const_match_arm_body_type(arm.body(), expected);
        }
        self.push_typed_const_scope();
        let result = (|| {
            self.bind_typed_resolved_const_patterns(arm.patterns(), target_ty)?;
            self.resolved_const_match_arm_body_type(arm.body(), expected)
        })();
        self.pop_typed_const_scope();
        result
    }

    pub(super) fn resolved_const_match_arm_body_type(
        &mut self,
        body: &ResolvedConstMatchArmBody,
        expected: Option<InternedTyId>,
    ) -> Option<ConstArmType> {
        match body.kind() {
            ResolvedConstMatchArmBodyKind::Expr(expr) => self
                .resolved_const_expr_type(expr, expected)
                .map(ConstArmType::Value),
            ResolvedConstMatchArmBodyKind::Block(block) => {
                self.resolved_const_match_block_arm_type(block, expected)
            }
            ResolvedConstMatchArmBodyKind::Stmt(stmt) => {
                self.resolved_const_stmt_arm_type(stmt, expected)
            }
        }
    }

    pub(super) fn resolved_const_match_block_arm_type(
        &mut self,
        block: &ResolvedConstBlock,
        expected: Option<InternedTyId>,
    ) -> Option<ConstArmType> {
        self.resolved_const_block_tail_type(block, expected)
            .map(ConstArmType::Value)
    }

    pub(super) fn resolved_const_stmt_arm_type(
        &mut self,
        stmt: &nia_const_ir::ResolvedConstStmt,
        expected: Option<InternedTyId>,
    ) -> Option<ConstArmType> {
        match stmt.kind() {
            ResolvedConstStmtKind::Expr(expr) => self
                .resolved_const_expr_type(expr, expected)
                .map(ConstArmType::Value),
            ResolvedConstStmtKind::Return(_)
            | ResolvedConstStmtKind::Break
            | ResolvedConstStmtKind::Continue => Some(ConstArmType::ControlFlow),
            ResolvedConstStmtKind::Binding(_)
            | ResolvedConstStmtKind::PatternBinding(_)
            | ResolvedConstStmtKind::If { .. }
            | ResolvedConstStmtKind::ForIn(_)
            | ResolvedConstStmtKind::While { .. }
            | ResolvedConstStmtKind::Loop { .. } => None,
        }
    }

    pub(super) fn resolved_const_if_expr_type(
        &mut self,
        cond: &ResolvedConstExpr,
        then_branch: &ResolvedConstBlock,
        else_branch: Option<&ResolvedConstExpr>,
        expected: Option<InternedTyId>,
    ) -> Option<ConstValueType> {
        self.check_resolved_const_bool_condition(cond)?;
        let expected = expected.and_then(|expected| self.usable_const_expected_type(expected));
        let else_branch = else_branch?;
        if let Some(expected) = expected {
            let then_ty = self
                .resolved_const_block_tail_runtime_type(then_branch, Some(expected))
                .or_else(|| self.resolved_const_block_tail_runtime_type(then_branch, None))?;
            let else_ty = self
                .resolved_const_arg_runtime_type(else_branch, Some(expected))
                .filter(|else_ty| *else_ty == then_ty)
                .or_else(|| self.resolved_const_arg_runtime_type(else_branch, Some(then_ty)))?;
            if then_ty != else_ty {
                if self.const_runtime_type_is_known(then_ty)
                    && self.const_runtime_type_is_known(else_ty)
                {
                    self.push_const_type_error(
                        else_branch.span(),
                        "const if branches have incompatible types",
                    );
                }
                return None;
            }
            return Some(ConstValueType::Runtime(then_ty));
        }
        let then_ty = self.resolved_const_block_tail_type(then_branch, None)?;
        let else_ty = self.resolved_const_expr_type(else_branch, None)?;
        if then_ty != else_ty {
            if self.const_value_types_have_known_mismatch(&then_ty, &else_ty) {
                self.push_const_type_error(
                    else_branch.span(),
                    "const if branches have incompatible types",
                );
            }
            return None;
        }
        Some(then_ty)
    }

    pub(super) fn usable_const_expected_type(&self, ty: InternedTyId) -> Option<InternedTyId> {
        match self.ty_kind(ty) {
            Some(TyKind::GenericParam(_)) => None,
            _ => Some(ty),
        }
    }

    pub(super) fn resolved_const_block_tail_runtime_type(
        &mut self,
        block: &ResolvedConstBlock,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        self.resolved_const_block_tail_type(block, expected)
            .and_then(|ty| ty.runtime())
    }

    pub(super) fn resolved_const_block_tail_type(
        &mut self,
        block: &ResolvedConstBlock,
        expected: Option<InternedTyId>,
    ) -> Option<ConstValueType> {
        if block.is_empty() {
            return self.resolved_const_expr_type(block.tail()?, expected);
        }
        self.push_typed_const_scope();
        let result = (|| {
            for stmt in block.stmts() {
                self.bind_typed_resolved_const_stmt(stmt)?;
            }
            self.resolved_const_expr_type(block.tail()?, expected)
        })();
        self.pop_typed_const_scope();
        result
    }

    pub(super) fn bind_typed_resolved_const_stmt(
        &mut self,
        stmt: &nia_const_ir::ResolvedConstStmt,
    ) -> Option<()> {
        match stmt.kind() {
            ResolvedConstStmtKind::Binding(binding) => {
                let explicit = binding
                    .explicit_type()
                    .map(|ty| self.substitute_ty_generics(ty));
                let inferred = self.resolved_const_expr_type(binding.value(), explicit);
                if let (Some(expected), Some(ConstValueType::Runtime(actual))) =
                    (explicit, &inferred)
                    && !self.const_function_types_match(expected, *actual)
                {
                    self.push_const_function_type_mismatch(
                        binding.value().span(),
                        "const binding initializer",
                    );
                }
                let ty = explicit.map(ConstValueType::Runtime).or(inferred)?;
                self.bind_const_local_type(binding.local_id(), ty, binding.is_mutable());
                Some(())
            }
            ResolvedConstStmtKind::PatternBinding(binding) => {
                self.bind_typed_resolved_const_pattern_binding(binding)
            }
            ResolvedConstStmtKind::Expr(expr) => {
                let _ = self.resolved_const_expr_type(expr, None);
                Some(())
            }
            ResolvedConstStmtKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.check_resolved_const_bool_condition(cond)?;
                self.check_resolved_const_block(then_branch)?;
                if let Some(else_branch) = else_branch {
                    self.check_resolved_const_block(else_branch)?;
                }
                Some(())
            }
            ResolvedConstStmtKind::ForIn(for_in) => self.check_resolved_const_for_in_stmt(for_in),
            ResolvedConstStmtKind::While { cond, body } => {
                self.check_resolved_const_bool_condition(cond)?;
                self.check_resolved_const_block(body)
            }
            ResolvedConstStmtKind::Loop { body } => self.check_resolved_const_block(body),
            ResolvedConstStmtKind::Return(_)
            | ResolvedConstStmtKind::Break
            | ResolvedConstStmtKind::Continue => None,
        }
    }

    fn bind_typed_resolved_const_pattern_binding(
        &mut self,
        binding: &ResolvedConstPatternBinding,
    ) -> Option<()> {
        let explicit = binding
            .explicit_type()
            .map(|ty| self.substitute_ty_generics(ty));
        let inferred = self.resolved_const_expr_type(binding.value(), explicit);
        if let (Some(expected), Some(ConstValueType::Runtime(actual))) = (explicit, &inferred)
            && !self.const_function_types_match(expected, *actual)
        {
            self.push_const_function_type_mismatch(
                binding.value().span(),
                "const pattern binding initializer",
            );
        }
        let target_ty = explicit.or(match inferred {
            Some(ConstValueType::Runtime(ty)) => Some(ty),
            _ => None,
        })?;
        self.check_resolved_const_patterns(std::slice::from_ref(binding.pattern()), target_ty)?;
        self.bind_typed_resolved_const_pattern(binding.pattern(), target_ty, binding.is_mutable())
    }

    pub(super) fn check_resolved_const_block(&mut self, block: &ResolvedConstBlock) -> Option<()> {
        self.push_typed_const_scope();
        let result = (|| {
            for stmt in block.stmts() {
                self.check_resolved_const_stmt(stmt)?;
            }
            if let Some(tail) = block.tail() {
                self.resolved_const_expr_type(tail, None)?;
            }
            Some(())
        })();
        self.pop_typed_const_scope();
        result
    }

    fn check_const_function_result(
        &mut self,
        expr: &ResolvedConstExpr,
        expected: Option<InternedTyId>,
        context: &str,
    ) -> Option<()> {
        let expected = expected.map(|ty| self.substitute_ty_generics(ty));
        let actual = self.resolved_const_expr_type(expr, expected);
        if let (Some(expected), Some(ConstValueType::Runtime(actual))) = (expected, actual)
            && !self.const_function_types_match(expected, actual)
        {
            self.push_const_function_type_mismatch(expr.span(), context);
        }
        Some(())
    }

    fn push_const_function_type_mismatch(&mut self, span: Span, context: &str) {
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            span,
            format!("{context} does not match its declared type"),
        ));
    }

    pub(super) fn const_function_types_match(
        &mut self,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> bool {
        let module_id = self.current_execution_module_id();
        let expected = self
            .type_normalization_for_module(module_id)
            .map(|normalization| normalization.as_ref().normalize(expected))
            .unwrap_or(expected);
        let actual = self
            .type_normalization_for_module(module_id)
            .map(|normalization| normalization.as_ref().normalize(actual))
            .unwrap_or(actual);
        if expected == actual {
            return true;
        }
        match (self.ty_kind(expected), self.ty_kind(actual)) {
            (
                Some(TyKind::Array {
                    len: expected_len,
                    elem: expected_elem,
                }),
                Some(TyKind::Array {
                    len: actual_len,
                    elem: actual_elem,
                }),
            ) => {
                let lengths_match = expected_len == actual_len
                    || matches!(
                        (
                            self.array_len_const_value(expected_len),
                            self.array_len_const_value(actual_len),
                        ),
                        (Some(expected), Some(actual)) if expected == actual
                    );
                lengths_match && self.const_function_types_match(expected_elem, actual_elem)
            }
            (
                Some(TyKind::Optional {
                    elem: expected_elem,
                }),
                Some(TyKind::Optional { elem: actual_elem }),
            )
            | (
                Some(TyKind::SlicePointee {
                    elem: expected_elem,
                }),
                Some(TyKind::SlicePointee { elem: actual_elem }),
            ) => self.const_function_types_match(expected_elem, actual_elem),
            (
                Some(TyKind::Pointer {
                    is_readonly: expected_readonly,
                    elem: expected_elem,
                }),
                Some(TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }),
            )
            | (
                Some(TyKind::VolatilePointer {
                    is_readonly: expected_readonly,
                    elem: expected_elem,
                }),
                Some(TyKind::VolatilePointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }),
            )
            | (
                Some(TyKind::Slice {
                    is_readonly: expected_readonly,
                    elem: expected_elem,
                }),
                Some(TyKind::Slice {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }),
            ) => {
                expected_readonly == actual_readonly
                    && self.const_function_types_match(expected_elem, actual_elem)
            }
            (
                Some(TyKind::ErrorUnion {
                    error: expected_error,
                    value: expected_value,
                }),
                Some(TyKind::ErrorUnion {
                    error: actual_error,
                    value: actual_value,
                }),
            ) => {
                self.const_function_types_match(expected_error, actual_error)
                    && self.const_function_types_match(expected_value, actual_value)
            }
            (
                Some(TyKind::Nominal {
                    def_id: expected_def,
                    args: expected_args,
                    const_args: expected_const_args,
                }),
                Some(TyKind::Nominal {
                    def_id: actual_def,
                    args: actual_args,
                    const_args: actual_const_args,
                }),
            ) => {
                expected_def == actual_def
                    && expected_args.len() == actual_args.len()
                    && expected_args
                        .into_iter()
                        .zip(actual_args)
                        .all(|(expected, actual)| self.const_function_types_match(expected, actual))
                    && self.const_generic_arg_slices_match_for_execution(
                        &expected_const_args,
                        &actual_const_args,
                    )
            }
            _ => false,
        }
    }

    fn const_generic_arg_slices_match_for_execution(
        &mut self,
        expected: &[ConstGenericArg],
        actual: &[ConstGenericArg],
    ) -> bool {
        expected.len() == actual.len()
            && expected.iter().zip(actual).all(|(expected, actual)| {
                self.const_function_types_match(expected.ty, actual.ty)
                    && self.const_generic_values_match_for_execution(expected, actual)
            })
    }

    pub(super) fn const_generic_values_match_for_execution(
        &mut self,
        expected: &ConstGenericArg,
        actual: &ConstGenericArg,
    ) -> bool {
        if expected.value == actual.value {
            return true;
        }
        match (
            self.resolve_const_generic_arg_for_execution(expected),
            self.resolve_const_generic_arg_for_execution(actual),
        ) {
            (Some(ConstGenericValue::Int(expected)), Some(ConstGenericValue::Int(actual))) => {
                expected.bits() == actual.bits()
            }
            (Some(expected), Some(actual)) => expected == actual,
            _ => false,
        }
    }

    pub(super) fn resolve_const_generic_arg_for_execution(
        &mut self,
        arg: &ConstGenericArg,
    ) -> Option<ConstGenericValue> {
        let ConstGenericValue::ConstExpr(id) = arg.value else {
            return (!matches!(arg.value, ConstGenericValue::GenericParam(_)))
                .then(|| arg.value.clone());
        };
        if matches!(
            self.ty_kind(arg.ty),
            Some(TyKind::Primitive(PrimitiveTy::Usize))
        ) && let Some(value) = self
            .array_lengths
            .get(&id)
            .copied()
            .or_else(|| self.eval_array_len_const_expr_id(id))
        {
            return Some(ConstGenericValue::Int(IntConst::unsigned(value.into())));
        }
        let expr = if id.module_id == self.input.defs.module_id {
            self.input.module.const_exprs().get(&id)?.clone()
        } else {
            (self.input.program.module?)(id.module_id)?
                .const_exprs()
                .get(&id)?
                .clone()
        };
        self.with_execution_module(id.module_id, |this| {
            this.const_generic_arg_from_resolved_expr(&expr, arg.ty, id.module_id)
                .ok()
        })
    }

    pub(super) fn check_resolved_const_stmt(
        &mut self,
        stmt: &nia_const_ir::ResolvedConstStmt,
    ) -> Option<()> {
        match stmt.kind() {
            ResolvedConstStmtKind::Binding(_)
            | ResolvedConstStmtKind::PatternBinding(_)
            | ResolvedConstStmtKind::If { .. }
            | ResolvedConstStmtKind::ForIn(_)
            | ResolvedConstStmtKind::While { .. }
            | ResolvedConstStmtKind::Loop { .. } => self.bind_typed_resolved_const_stmt(stmt),
            ResolvedConstStmtKind::Expr(expr) => {
                let _ = self.resolved_const_expr_type(expr, None);
                Some(())
            }
            ResolvedConstStmtKind::Break | ResolvedConstStmtKind::Continue => Some(()),
            ResolvedConstStmtKind::Return(Some(expr)) => {
                let return_type = self
                    .active_execution_frames()
                    .find_map(|frame| frame.return_type);
                self.check_const_function_result(expr, return_type, "const return value")
            }
            ResolvedConstStmtKind::Return(None) => {
                let return_type = self
                    .active_execution_frames()
                    .find_map(|frame| frame.return_type);
                let return_type = return_type.map(|ty| self.substitute_ty_generics(ty));
                if return_type
                    .is_some_and(|ty| !self.ty_kind(ty).is_some_and(|kind| kind.is_unit()))
                {
                    self.push_const_function_type_mismatch(stmt.span(), "const return value");
                }
                Some(())
            }
        }
    }

    pub(super) fn check_resolved_const_for_in_stmt(
        &mut self,
        for_in: &nia_const_ir::ResolvedConstForIn,
    ) -> Option<()> {
        let iter_ty = self.resolved_const_expr_type(for_in.iter(), None)?;
        let Some(binding_ty) = self.const_for_in_binding_type(iter_ty) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                for_in.iter().span(),
                "const for-in expects an Iterable".to_string(),
            ));
            return None;
        };
        self.push_typed_const_scope();
        let result = (|| {
            let ConstValueType::Runtime(binding_ty) = binding_ty else {
                return None;
            };
            self.bind_typed_resolved_const_pattern(for_in.pattern(), binding_ty, false)?;
            for stmt in for_in.body().stmts() {
                self.check_resolved_const_stmt(stmt)?;
            }
            if let Some(tail) = for_in.body().tail() {
                self.resolved_const_expr_type(tail, None)?;
            }
            Some(())
        })();
        self.pop_typed_const_scope();
        result
    }

    pub(super) fn const_for_in_binding_type(
        &mut self,
        iter_ty: ConstValueType,
    ) -> Option<ConstValueType> {
        let ConstValueType::Runtime(iter_ty) = iter_ty else {
            return None;
        };
        let iter_ty = self.type_for_module_or_none(iter_ty, self.current_execution_module_id())?;
        if !self.proves_trait_obligation(
            iter_ty,
            TraitId::Builtin(nia_ty::BuiltinTrait::Iterable),
            Vec::new(),
        ) {
            return None;
        }
        let item = self.intern_current_ty(TyKind::Projection {
            self_ty: iter_ty,
            trait_id: TraitId::Builtin(nia_ty::BuiltinTrait::Iterable),
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            name: nia_symbol::known::ITEM,
        })?;
        Some(ConstValueType::Runtime(self.normalize_projection(item)))
    }

    pub(super) fn intern_current_ty(&mut self, kind: TyKind) -> Option<InternedTyId> {
        let module_id = self.current_execution_module_id();
        self.ensure_type_context(module_id)?;
        self.type_contexts
            .get(&module_id)
            .map(|types| types.intern(kind))
    }
}
