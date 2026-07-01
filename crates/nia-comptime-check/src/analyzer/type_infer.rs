use super::ty_substitution::substitute_ty_generics_in_interner;
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ComptimeTypeCompatibility {
    Mismatch,
    Unknown,
}

impl Analyzer<'_> {
    pub(super) fn substitute_ty_generics(&mut self, ty: InternedTyId) -> InternedTyId {
        let module_id = self.current_execution_module_id();
        let substitutions = self
            .call_locals
            .iter()
            .flat_map(|frame| frame.type_substitutions.iter())
            .map(|(name, ty)| (name.clone(), *ty))
            .collect::<HashMap<_, _>>();
        let interner = self
            .working_interners
            .get_mut(&module_id)
            .expect("working interner must exist for current execution module");
        substitute_ty_generics_in_interner(interner, ty, &|name| substitutions.get(name).copied())
    }

    pub(super) fn instantiate_resolved_function_generics(
        &mut self,
        span: Span,
        signature_module_id: ModuleId,
        signature: &FunctionSignature,
        type_args: &[ResolvedComptimeTypeArg],
        arg_exprs: &[ResolvedComptimeExpr],
        expected_return: Option<InternedTyId>,
    ) -> Result<ComptimeGenericInstantiation, ComptimeError> {
        if self.ensure_working_interner(signature_module_id).is_none() {
            return Err(ComptimeError {
                span,
                message: "cannot instantiate comptime function without module type interner"
                    .to_string(),
            });
        }
        if !type_args.is_empty()
            && let ArityCheck::Mismatch { actual, .. } =
                check_exact_arity(signature.generics.len(), type_args.len())
        {
            return Err(ComptimeError {
                span,
                message: format!(
                    "generic argument count mismatch for comptime function: expected {}, got {}",
                    signature.generics.len(),
                    actual
                ),
            });
        }
        let mut substitutions = HashMap::new();
        let mut const_substitutions = HashMap::new();
        if type_args.is_empty() {
            if let Some(expected) = expected_return
                && let Some(expected) =
                    self.import_ty_into_module_or_none(expected, signature_module_id)
            {
                self.infer_generics_from_tys(
                    span,
                    signature_module_id,
                    signature.return_type,
                    expected,
                    &mut substitutions,
                )?;
            }
            for (param, arg_expr) in signature.params.iter().zip(arg_exprs) {
                let expected = self.comptime_expected_param_type(
                    signature_module_id,
                    param.ty,
                    &substitutions,
                );
                let concrete_expected =
                    expected.filter(|expected| !self.type_contains_generic(*expected));
                let Some(arg_ty) =
                    self.resolved_comptime_arg_runtime_type(arg_expr, concrete_expected)
                else {
                    if concrete_expected.is_some_and(|expected| {
                        self.resolved_comptime_arg_expected_compatibility(arg_expr, expected)
                            == ComptimeTypeCompatibility::Mismatch
                    }) {
                        return Err(ComptimeError {
                            span: arg_expr.span(),
                            message: match arg_expr.kind() {
                                ResolvedComptimeExprKind::Switch(_) => {
                                    "comptime switch expression does not match expected type"
                                }
                                _ => "comptime call argument does not match expected type",
                            }
                            .to_string(),
                        });
                    }
                    continue;
                };
                self.infer_generics_from_tys(
                    span,
                    signature_module_id,
                    param.ty,
                    arg_ty,
                    &mut substitutions,
                )?;
            }
            for generic in &signature.generics {
                if !substitutions.contains_key(generic) {
                    return Err(ComptimeError {
                        span,
                        message: format!("cannot infer comptime generic type argument `{generic}`"),
                    });
                }
            }
        } else {
            for (generic, arg) in signature.generic_params.iter().zip(type_args) {
                match &generic.kind {
                    GenericParamSignatureKind::Type => {
                        let imported = self.import_ty_into_module(arg.ty(), signature_module_id)?;
                        substitutions.insert(generic.name.clone(), imported);
                    }
                    GenericParamSignatureKind::Comptime { ty } => {
                        let value = self
                            .const_generic_arg_from_resolved_type_arg(arg, signature_module_id)?;
                        const_substitutions.insert(
                            generic.name.clone(),
                            nia_ty::ConstGenericArg { ty: *ty, value },
                        );
                    }
                }
            }
        }
        Ok(ComptimeGenericInstantiation {
            type_substitutions: substitutions,
            const_substitutions,
        })
    }

    fn const_generic_arg_from_resolved_type_arg(
        &mut self,
        arg: &ResolvedComptimeTypeArg,
        module_id: ModuleId,
    ) -> Result<nia_ty::ConstGenericValue, ComptimeError> {
        let imported = self.import_ty_into_module(arg.ty(), module_id)?;
        match self
            .working_interners
            .get(&module_id)
            .and_then(|interner| interner.get(imported))
        {
            Some(TyKind::GenericParam(name)) => {
                Ok(nia_ty::ConstGenericValue::GenericParam(name.clone()))
            }
            _ => Err(ComptimeError {
                span: arg.span(),
                message: "comptime generic argument must be a comptime value".to_string(),
            }),
        }
    }

    pub(super) fn comptime_expected_param_type(
        &mut self,
        module_id: ModuleId,
        ty: InternedTyId,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Option<InternedTyId> {
        self.ensure_working_interner(module_id)?;
        let interner = self.working_interners.get_mut(&module_id)?;
        Some(substitute_ty_generics_in_interner(
            interner,
            ty,
            &|generic| substitutions.get(generic).copied(),
        ))
    }

    pub(super) fn resolved_comptime_arg_runtime_type(
        &mut self,
        expr: &ResolvedComptimeExpr,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        self.resolved_comptime_expr_type(expr, expected)
            .and_then(|ty| ty.runtime())
    }

    fn resolved_comptime_arg_expected_compatibility(
        &mut self,
        expr: &ResolvedComptimeExpr,
        expected: InternedTyId,
    ) -> ComptimeTypeCompatibility {
        match expr.kind() {
            ResolvedComptimeExprKind::Cast { expr: inner, ty } => {
                match self.resolved_comptime_cast_expected_compatibility(inner, *ty) {
                    ComptimeTypeCompatibility::Mismatch => ComptimeTypeCompatibility::Mismatch,
                    ComptimeTypeCompatibility::Unknown => ComptimeTypeCompatibility::Unknown,
                }
            }
            ResolvedComptimeExprKind::Switch(switch)
                if self.resolved_comptime_switch_has_definite_pattern_mismatch(switch) =>
            {
                ComptimeTypeCompatibility::Mismatch
            }
            _ => {
                let _ = expected;
                ComptimeTypeCompatibility::Unknown
            }
        }
    }

    fn resolved_comptime_cast_expected_compatibility(
        &mut self,
        inner: &ResolvedComptimeExpr,
        target: InternedTyId,
    ) -> ComptimeTypeCompatibility {
        let target = self.substitute_ty_generics(target);
        let Some(TyKind::Primitive(target)) = self.ty_kind(target) else {
            return ComptimeTypeCompatibility::Mismatch;
        };
        let Some(source) = self.resolved_comptime_expr_type(inner, None) else {
            return ComptimeTypeCompatibility::Unknown;
        };
        let ComptimeValueType::Runtime(source) = source else {
            return ComptimeTypeCompatibility::Mismatch;
        };
        let Some(TyKind::Primitive(source)) = self.ty_kind(source) else {
            return ComptimeTypeCompatibility::Mismatch;
        };
        let source_numeric = primitive_integer_layout(source, self.input.target.pointer_width)
            .is_some()
            || is_float_primitive(source);
        let target_numeric = primitive_integer_layout(target, self.input.target.pointer_width)
            .is_some()
            || is_float_primitive(target);
        if source_numeric && target_numeric {
            ComptimeTypeCompatibility::Unknown
        } else {
            ComptimeTypeCompatibility::Mismatch
        }
    }

    pub(super) fn probe_resolved_comptime_int_expr(
        &mut self,
        expr: &ResolvedComptimeExpr,
    ) -> Option<i128> {
        nia_comptime_engine::eval_resolved_comptime_int_expr(expr, self)
            .ok()
            .and_then(IntConst::as_i128)
    }

    pub(super) fn probe_resolved_comptime_array_len_expr(
        &mut self,
        expr: &ResolvedComptimeExpr,
    ) -> Option<u64> {
        nia_comptime_engine::eval_resolved_comptime_array_len_expr(expr, self).ok()
    }

    pub(super) fn probe_type_generic_inference(
        &mut self,
        span: Span,
        expected: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
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

    pub(super) fn comptime_name_resolution_type(
        &mut self,
        resolution: ComptimeNameResolution,
    ) -> Option<ComptimeValueType> {
        match resolution {
            ComptimeNameResolution::Local(local_id) => self
                .call_local_type(local_id)
                .or_else(|| {
                    let ty = self
                        .typed_value_for_key(ComptimeKey::Local(local_id))
                        .map(|typed| typed.ty.clone())?;
                    self.import_comptime_value_type(ty, self.current_execution_module_id())
                })
                .or_else(|| {
                    self.explicit_type_for_key(ComptimeKey::Local(local_id))
                        .and_then(|ty| {
                            self.import_ty_into_module_or_none(
                                ty,
                                self.current_execution_module_id(),
                            )
                        })
                        .map(ComptimeValueType::Runtime)
                }),
            ComptimeNameResolution::Global(global_id) => self
                .typed_value_for_key(ComptimeKey::Global(global_id))
                .map(|typed| typed.ty.clone())
                .and_then(|ty| {
                    self.import_comptime_value_type(ty, self.current_execution_module_id())
                })
                .or_else(|| {
                    self.explicit_type_for_key(ComptimeKey::Global(global_id))
                        .and_then(|ty| {
                            self.import_ty_into_module_or_none(
                                ty,
                                self.current_execution_module_id(),
                            )
                        })
                        .map(ComptimeValueType::Runtime)
                }),
            ComptimeNameResolution::BuiltinAssociatedValue(value) => {
                let BuiltinAssociatedValue::PrimitiveIntLimit { primitive, .. } = value;
                Some(ComptimeValueType::Runtime(
                    self.source_interner_for_module(self.current_execution_module_id())
                        .unwrap_or_else(|| self.input.interner.clone())
                        .primitive(primitive),
                ))
            }
            ComptimeNameResolution::AssociatedComptimeProjection(projection) => self
                .associated_comptime_projection_type(&projection)
                .map(ComptimeValueType::Runtime),
            ComptimeNameResolution::GenericParam(name) => self
                .call_locals
                .iter()
                .rev()
                .find_map(|frame| frame.const_substitutions.get(&name))
                .map(|arg| ComptimeValueType::Runtime(arg.ty)),
        }
    }

    pub(super) fn resolved_comptime_expr_type(
        &mut self,
        expr: &ResolvedComptimeExpr,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
        match expr.kind() {
            ResolvedComptimeExprKind::Name(resolution) => {
                self.comptime_name_resolution_type(resolution.clone())
            }
            ResolvedComptimeExprKind::Integer(text) => {
                integer_literal_suffix_ty(text).map(|primitive| {
                    ComptimeValueType::Runtime(
                        self.source_interner_for_module(self.current_execution_module_id())
                            .unwrap_or_else(|| self.input.interner.clone())
                            .primitive(primitive),
                    )
                })
            }
            ResolvedComptimeExprKind::Float(text) => {
                let primitive = float_literal_suffix_ty(text).unwrap_or(PrimitiveTy::F64);
                Some(ComptimeValueType::Runtime(
                    self.current_runtime_primitive_type(primitive),
                ))
            }
            ResolvedComptimeExprKind::Char(_) => Some(ComptimeValueType::Runtime(
                self.current_runtime_primitive_type(PrimitiveTy::Char),
            )),
            ResolvedComptimeExprKind::ByteChar(_) => Some(ComptimeValueType::Runtime(
                self.current_runtime_primitive_type(PrimitiveTy::U8),
            )),
            ResolvedComptimeExprKind::String(literal) => self.comptime_string_literal_type(literal),
            ResolvedComptimeExprKind::ByteString(literal) => self
                .comptime_byte_string_literal_type(
                    nia_comptime_engine::eval_byte_string_literal(literal)?.len() as u64,
                ),
            ResolvedComptimeExprKind::Embed { path } => {
                let path = nia_comptime_engine::eval_string_literal(path)?;
                let resolved = super::env_impl::resolve_embed_path(
                    self.current_execution_source_path()?.as_str(),
                    &path,
                );
                let len = std::fs::metadata(resolved).ok()?.len();
                self.comptime_byte_string_literal_type(len)
            }
            ResolvedComptimeExprKind::Bool(_) => Some(ComptimeValueType::Runtime(
                self.current_runtime_primitive_type(PrimitiveTy::Bool),
            )),
            ResolvedComptimeExprKind::ArrayLiteral { ty: Some(ty), .. }
            | ResolvedComptimeExprKind::StructLiteral { ty: Some(ty), .. } => {
                Some(ComptimeValueType::Runtime(*ty))
            }
            ResolvedComptimeExprKind::ArrayLiteral { ty: None, elems } => {
                self.resolved_comptime_array_literal_type(elems, expected)
            }
            ResolvedComptimeExprKind::StructLiteral { ty: None, fields } => {
                self.resolved_comptime_struct_literal_type(expr.span(), fields, expected)
            }
            ResolvedComptimeExprKind::OptionalSome { expr: inner } => {
                let expected_elem = expected.and_then(|expected| match self.ty_kind(expected) {
                    Some(TyKind::Optional { elem }) => Some(elem),
                    _ => None,
                });
                let elem = self.resolved_comptime_arg_runtime_type(inner, expected_elem)?;
                self.comptime_runtime_type(
                    elem,
                    |elem| TyKind::Optional { elem },
                    self.current_execution_module_id(),
                )
                .map(ComptimeValueType::Runtime)
            }
            ResolvedComptimeExprKind::ErrorOk { expr: inner } => {
                let (error, value) = self.expected_error_union_parts(expected?)?;
                let actual_value = self.resolved_comptime_arg_runtime_type(inner, Some(value))?;
                self.comptime_error_union_type(error, actual_value)
                    .map(ComptimeValueType::Runtime)
            }
            ResolvedComptimeExprKind::ErrorErr { expr: inner } => {
                let (error, value) = self.expected_error_union_parts(expected?)?;
                let actual_error = self.resolved_comptime_arg_runtime_type(inner, Some(error))?;
                self.comptime_error_union_type(actual_error, value)
                    .map(ComptimeValueType::Runtime)
            }
            ResolvedComptimeExprKind::Try { expr: inner } => {
                let inner_ty = self.resolved_comptime_arg_runtime_type(inner, None)?;
                let payload = match self.ty_kind(inner_ty)? {
                    TyKind::Optional { elem } => elem,
                    TyKind::ErrorUnion { value, .. } => value,
                    _ => return None,
                };
                self.import_ty_into_module_or_none(payload, self.current_execution_module_id())
                    .map(ComptimeValueType::Runtime)
            }
            ResolvedComptimeExprKind::Field { lhs, name } => {
                let lhs_ty = self.resolved_comptime_expr_type(lhs, None)?;
                self.comptime_field_type(lhs_ty, name)
            }
            ResolvedComptimeExprKind::BuiltinMethod { method, lhs } => {
                let lhs_ty = self.resolved_comptime_expr_type(lhs, None)?;
                self.comptime_builtin_method_type(*method, lhs_ty)
            }
            ResolvedComptimeExprKind::Cast { expr: inner, ty } => {
                self.resolved_comptime_cast_type(inner, *ty)
            }
            ResolvedComptimeExprKind::Index { lhs, index } => {
                let lhs_ty = self.resolved_comptime_expr_type(lhs, None)?;
                self.resolved_comptime_index_type(expr.span(), lhs_ty, index)
            }
            ResolvedComptimeExprKind::Slice { lhs, range } => {
                let lhs_ty = self.resolved_comptime_expr_type(lhs, None)?;
                self.resolved_comptime_slice_type(lhs_ty, range, expected)
            }
            ResolvedComptimeExprKind::Range(range) => {
                self.resolved_comptime_range_type(range, expected)
            }
            ResolvedComptimeExprKind::Binary { lhs, op, rhs } => {
                self.resolved_comptime_binary_expr_type(lhs, *op, rhs)
            }
            ResolvedComptimeExprKind::Unary { op, expr: inner } => {
                self.resolved_comptime_unary_expr_type(*op, inner)
            }
            ResolvedComptimeExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.resolved_comptime_if_expr_type(
                cond,
                then_branch,
                else_branch.as_deref(),
                expected,
            ),
            ResolvedComptimeExprKind::Switch(switch) => {
                self.resolved_comptime_switch_expr_type(switch, expected)
            }
            ResolvedComptimeExprKind::Block(block) => {
                self.resolved_comptime_block_tail_type(block, expected)
            }
            ResolvedComptimeExprKind::BuiltinValue(ValueBuiltin::Error) => None,
            ResolvedComptimeExprKind::CompileError { message } => {
                let _ = self.resolved_comptime_expr_type(message, None);
                expected.map(ComptimeValueType::Runtime)
            }
            ResolvedComptimeExprKind::Call {
                callee,
                type_args,
                args,
            } => self
                .resolved_comptime_call_return_type(expr.span(), callee, type_args, args, expected)
                .map(ComptimeValueType::Runtime),
            ResolvedComptimeExprKind::LayoutBuiltin { .. }
            | ResolvedComptimeExprKind::FieldOffsetBuiltin { .. }
            | ResolvedComptimeExprKind::Null
            | ResolvedComptimeExprKind::Assign(_) => None,
        }
    }

    pub(super) fn find_resolved_pattern_local_type(
        &mut self,
        switch: &ResolvedComptimeSwitch,
        local_id: LocalId,
    ) -> Option<InternedTyId> {
        let target_ty = self.resolved_comptime_arg_runtime_type(switch.target(), None)?;
        for arm in switch.arms() {
            for pattern in arm.patterns() {
                if resolved_pattern_local_id(pattern) == Some(local_id) {
                    return self.resolved_pattern_binding_type(pattern, target_ty);
                }
            }
        }
        None
    }

    pub(super) fn resolved_pattern_binding_type(
        &self,
        pattern: &ResolvedComptimePattern,
        target_ty: InternedTyId,
    ) -> Option<InternedTyId> {
        match pattern.kind() {
            ResolvedComptimePatternKind::Bind { .. } => Some(target_ty),
            ResolvedComptimePatternKind::Pointer { pattern, .. }
            | ResolvedComptimePatternKind::MutPointer { pattern, .. } => {
                let TyKind::Pointer { elem, .. } = self.ty_kind(target_ty)? else {
                    return None;
                };
                self.resolved_pattern_binding_type(pattern, elem)
            }
            ResolvedComptimePatternKind::OptionalSome { pattern, .. } => {
                let TyKind::Optional { elem } = self.ty_kind(target_ty)? else {
                    return None;
                };
                self.resolved_pattern_binding_type(pattern, elem)
            }
            ResolvedComptimePatternKind::ErrorOk { pattern, .. } => {
                let TyKind::ErrorUnion { value, .. } = self.ty_kind(target_ty)? else {
                    return None;
                };
                self.resolved_pattern_binding_type(pattern, value)
            }
            ResolvedComptimePatternKind::ErrorErr { pattern, .. } => {
                let TyKind::ErrorUnion { error, .. } = self.ty_kind(target_ty)? else {
                    return None;
                };
                self.resolved_pattern_binding_type(pattern, error)
            }
            ResolvedComptimePatternKind::Wildcard { .. }
            | ResolvedComptimePatternKind::OptionalNull { .. }
            | ResolvedComptimePatternKind::Expr(_)
            | ResolvedComptimePatternKind::Range { .. } => None,
        }
    }

    pub(super) fn comptime_field_type(
        &mut self,
        lhs: ComptimeValueType,
        name: &str,
    ) -> Option<ComptimeValueType> {
        match &lhs {
            ComptimeValueType::Struct(_) => lhs.structural_field(name).cloned(),
            ComptimeValueType::Runtime(ty) => self
                .comptime_nominal_struct_field_type(*ty, name)
                .map(ComptimeValueType::Runtime),
            ComptimeValueType::Array { .. }
            | ComptimeValueType::Int
            | ComptimeValueType::Bool
            | ComptimeValueType::String => None,
        }
    }

    pub(super) fn comptime_builtin_method_type(
        &mut self,
        method: BuiltinTraitMethod,
        lhs: ComptimeValueType,
    ) -> Option<ComptimeValueType> {
        let receiver_ty = match lhs {
            ComptimeValueType::Array { .. } if method == BuiltinTraitMethod::Len => {
                return Some(ComptimeValueType::Runtime(
                    self.current_runtime_primitive_type(PrimitiveTy::Usize),
                ));
            }
            ComptimeValueType::Runtime(ty) => ty,
            ComptimeValueType::Array { .. }
            | ComptimeValueType::Struct(_)
            | ComptimeValueType::Int
            | ComptimeValueType::Bool
            | ComptimeValueType::String => return None,
        };
        let trait_id = method.trait_id();
        if !matches!(
            method,
            BuiltinTraitMethod::Len | BuiltinTraitMethod::Start | BuiltinTraitMethod::End
        ) || !self.proves_trait_obligation(receiver_ty, TraitId::Builtin(trait_id), Vec::new())
        {
            return None;
        }
        match method {
            BuiltinTraitMethod::Len => Some(ComptimeValueType::Runtime(
                self.current_runtime_primitive_type(PrimitiveTy::Usize),
            )),
            BuiltinTraitMethod::Start | BuiltinTraitMethod::End => self
                .resolve_associated_type_projection(
                    receiver_ty,
                    TraitId::Builtin(trait_id),
                    &[],
                    &[],
                    BuiltinAssociatedType::Output.name(),
                )
                .and_then(|ty| {
                    self.import_ty_into_module_or_none(ty, self.current_execution_module_id())
                })
                .map(ComptimeValueType::Runtime),
            _ => None,
        }
    }

    pub(super) fn resolved_comptime_index_type(
        &mut self,
        span: Span,
        lhs: ComptimeValueType,
        index: &ResolvedComptimeExpr,
    ) -> Option<ComptimeValueType> {
        match lhs {
            ComptimeValueType::Array { .. } => {
                let (elem, len) = lhs.array_elem()?;
                if let Some(len) = len
                    && self
                        .probe_resolved_comptime_array_len_expr(index)
                        .is_some_and(|index| index >= len)
                {
                    return None;
                }
                Some(elem.clone())
            }
            ComptimeValueType::Runtime(ty) => {
                self.resolved_comptime_runtime_index_type(span, ty, index)
            }
            ComptimeValueType::Struct(_)
            | ComptimeValueType::Int
            | ComptimeValueType::Bool
            | ComptimeValueType::String => None,
        }
    }

    pub(super) fn resolved_comptime_runtime_index_type(
        &mut self,
        _span: Span,
        lhs: InternedTyId,
        index: &ResolvedComptimeExpr,
    ) -> Option<ComptimeValueType> {
        let (len, elem) = match self.ty_kind(lhs)? {
            TyKind::Array { len, elem } => (Some(len), elem),
            TyKind::Slice { elem, .. } => (None, elem),
            _ => return None,
        };
        if let Some(ArrayLenTy::ConstValue(len)) = len {
            let index = self.probe_resolved_comptime_array_len_expr(index)?;
            if index >= len {
                return None;
            }
        } else {
            self.probe_resolved_comptime_int_expr(index)?;
        }
        self.import_ty_into_module_or_none(elem, self.current_execution_module_id())
            .map(ComptimeValueType::Runtime)
    }

    pub(super) fn resolved_comptime_slice_type(
        &mut self,
        lhs: ComptimeValueType,
        range: &nia_comptime_ir::ResolvedComptimeSliceRange,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
        match lhs {
            ComptimeValueType::Array { .. } => {
                let (elem, len) = lhs.array_elem()?;
                let expected_len = self.expected_const_array_len(expected);
                let actual_len = self.resolved_comptime_slice_len(len, expected_len, range)?;
                self.comptime_slice_result_type(elem.clone(), actual_len, expected)
            }
            ComptimeValueType::Runtime(ty) => {
                self.resolved_comptime_runtime_slice_type(ty, range, expected)
            }
            ComptimeValueType::Struct(_)
            | ComptimeValueType::Int
            | ComptimeValueType::Bool
            | ComptimeValueType::String => None,
        }
    }

    pub(super) fn resolved_comptime_runtime_slice_type(
        &mut self,
        lhs: InternedTyId,
        range: &nia_comptime_ir::ResolvedComptimeSliceRange,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
        let (len, elem) = match self.ty_kind(lhs)? {
            TyKind::Array { len, elem } => (Some(len), elem),
            TyKind::Slice { elem, .. } => (None, elem),
            _ => return None,
        };
        let known_len = len.and_then(|len| self.array_len_const_value(len));
        let expected_len = self.expected_const_array_len(expected);
        let actual_len = self.resolved_comptime_slice_len(known_len, expected_len, range)?;
        let elem = self.import_ty_into_module_or_none(elem, self.current_execution_module_id())?;
        self.comptime_slice_result_type(ComptimeValueType::Runtime(elem), actual_len, expected)
    }

    pub(super) fn comptime_slice_result_type(
        &mut self,
        elem: ComptimeValueType,
        actual_len: u64,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
        if let Some((expected_len, expected_elem)) =
            expected.and_then(|expected| self.expected_array_parts(expected))
            && elem.runtime() == Some(expected_elem)
        {
            let len = self.comptime_array_literal_len(Some(expected_len), Some(actual_len))?;
            return self
                .comptime_runtime_type(
                    expected_elem,
                    |elem| TyKind::Array { len, elem },
                    self.current_execution_module_id(),
                )
                .map(ComptimeValueType::Runtime);
        }
        Some(ComptimeValueType::Array {
            elem: Box::new(elem),
            len: Some(actual_len),
        })
    }

    pub(super) fn resolved_comptime_slice_len(
        &mut self,
        source_len: Option<u64>,
        expected_len: Option<u64>,
        range: &nia_comptime_ir::ResolvedComptimeSliceRange,
    ) -> Option<u64> {
        let start = match range.start() {
            Some(start) => self.probe_resolved_comptime_array_len_expr(start)?,
            None => 0,
        };
        let mut end = match range.end() {
            Some(end) => self.probe_resolved_comptime_array_len_expr(end)?,
            None => source_len.or_else(|| expected_len.and_then(|len| start.checked_add(len)))?,
        };
        if range.is_inclusive() {
            end = end.checked_add(1)?;
        }
        if start > end {
            return None;
        }
        if let Some(source_len) = source_len
            && end > source_len
        {
            return None;
        }
        Some(end - start)
    }

    pub(super) fn expected_const_array_len(
        &mut self,
        expected: Option<InternedTyId>,
    ) -> Option<u64> {
        let expected = expected?;
        let TyKind::Array { len, .. } = self.ty_kind(expected)? else {
            return None;
        };
        self.array_len_const_value(len)
    }

    pub(super) fn array_len_const_value(&mut self, len: ArrayLenTy) -> Option<u64> {
        match len {
            ArrayLenTy::ConstValue(len) => Some(len),
            ArrayLenTy::ConstExpr(id) => self
                .array_lengths
                .get(&id)
                .copied()
                .or_else(|| self.eval_array_len_const_expr_id(id)),
            ArrayLenTy::Infer | ArrayLenTy::GenericParam(_) | ArrayLenTy::Builtin { .. } => None,
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

    pub(super) fn import_comptime_value_type(
        &mut self,
        ty: ComptimeValueType,
        target_module_id: ModuleId,
    ) -> Option<ComptimeValueType> {
        match ty {
            ComptimeValueType::Runtime(ty) => self
                .import_ty_into_module_or_none(ty, target_module_id)
                .map(ComptimeValueType::Runtime),
            ComptimeValueType::Array { elem, len } => Some(ComptimeValueType::Array {
                elem: Box::new(self.import_comptime_value_type(*elem, target_module_id)?),
                len,
            }),
            ComptimeValueType::Struct(fields) => fields
                .into_iter()
                .map(|field| {
                    Some(ComptimeValueFieldType {
                        name: field.name,
                        ty: self.import_comptime_value_type(field.ty, target_module_id)?,
                    })
                })
                .collect::<Option<Vec<_>>>()
                .map(ComptimeValueType::Struct),
            ComptimeValueType::Int => Some(ComptimeValueType::Int),
            ComptimeValueType::Bool => Some(ComptimeValueType::Bool),
            ComptimeValueType::String => Some(ComptimeValueType::String),
        }
    }

    pub(super) fn resolved_comptime_array_literal_type(
        &mut self,
        elems: &ResolvedComptimeArrayElements,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
        let expected_parts = expected.and_then(|expected| self.expected_array_parts(expected));
        if expected_parts.is_none()
            && let Some(ty) = self.structural_resolved_comptime_array_literal_type(elems)
        {
            return Some(ty);
        }
        let (elem_ty, actual_len) = match elems.kind() {
            ResolvedComptimeArrayElementsKind::List(elems) => {
                let expected_elem = expected_parts.as_ref().map(|(_, elem)| *elem);
                let elem_ty = self.resolved_comptime_array_list_elem_type(elems, expected_elem)?;
                (elem_ty, Some(elems.len() as u64))
            }
            ResolvedComptimeArrayElementsKind::Repeat { value, count } => {
                let expected_elem = expected_parts.as_ref().map(|(_, elem)| *elem);
                let elem_ty = self.resolved_comptime_arg_runtime_type(value, expected_elem)?;
                let actual_len = Some(self.probe_resolved_comptime_array_len_expr(count)?);
                (elem_ty, actual_len)
            }
        };
        let len =
            self.comptime_array_literal_len(expected_parts.map(|(len, _)| len), actual_len)?;
        self.comptime_runtime_type(
            elem_ty,
            |elem| TyKind::Array { len, elem },
            self.current_execution_module_id(),
        )
        .map(ComptimeValueType::Runtime)
    }

    pub(super) fn structural_resolved_comptime_array_literal_type(
        &mut self,
        elems: &ResolvedComptimeArrayElements,
    ) -> Option<ComptimeValueType> {
        let (elem_ty, len) = match elems.kind() {
            ResolvedComptimeArrayElementsKind::List(elems) => {
                let first = elems.first()?;
                let elem_ty = self.resolved_comptime_expr_type(first, None)?;
                for elem in &elems[1..] {
                    if self.resolved_comptime_expr_type(elem, None)? != elem_ty {
                        return None;
                    }
                }
                (elem_ty, Some(elems.len() as u64))
            }
            ResolvedComptimeArrayElementsKind::Repeat { value, count } => {
                let elem_ty = self.resolved_comptime_expr_type(value, None)?;
                let len = self.probe_resolved_comptime_array_len_expr(count)?;
                (elem_ty, Some(len))
            }
        };
        Some(ComptimeValueType::Array {
            elem: Box::new(elem_ty),
            len,
        })
    }

    pub(super) fn expected_array_parts(
        &self,
        expected: InternedTyId,
    ) -> Option<(ArrayLenTy, InternedTyId)> {
        match self.ty_kind(expected)? {
            TyKind::Array { len, elem } => Some((len, elem)),
            _ => None,
        }
    }

    pub(super) fn resolved_comptime_array_list_elem_type(
        &mut self,
        elems: &[ResolvedComptimeExpr],
        expected_elem: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let (anchor_index, elem_ty) =
            self.resolved_comptime_array_list_anchor_elem_type(elems, expected_elem)?;
        for (index, elem) in elems.iter().enumerate() {
            if index == anchor_index {
                continue;
            }
            let actual = self.resolved_comptime_arg_runtime_type(elem, Some(elem_ty))?;
            if actual != elem_ty {
                return None;
            }
        }
        Some(elem_ty)
    }

    pub(super) fn resolved_comptime_array_list_anchor_elem_type(
        &mut self,
        elems: &[ResolvedComptimeExpr],
        expected_elem: Option<InternedTyId>,
    ) -> Option<(usize, InternedTyId)> {
        for (index, elem) in elems.iter().enumerate() {
            let expected_ty = expected_elem
                .and_then(|expected| self.resolved_comptime_arg_runtime_type(elem, Some(expected)))
                .filter(|ty| !self.type_contains_generic(*ty));
            if let Some(ty) =
                expected_ty.or_else(|| self.resolved_comptime_arg_runtime_type(elem, None))
                && !self.type_contains_generic(ty)
            {
                return Some((index, ty));
            }
        }
        None
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
            Some(TyKind::GenericParam(_)) => true,
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
                | TyKind::ComptimeOnly
                | TyKind::Primitive(_)
                | TyKind::BuiltinType(_)
                | TyKind::Vector { .. },
            )
            | None => false,
        }
    }

    pub(super) fn comptime_array_literal_len(
        &self,
        expected: Option<ArrayLenTy>,
        actual: Option<u64>,
    ) -> Option<ArrayLenTy> {
        match check_array_literal_len(expected, None, actual) {
            ArrayLiteralLenCheck::Accepted(len) => Some(len),
            ArrayLiteralLenCheck::Mismatch { .. } | ArrayLiteralLenCheck::Unknown => None,
        }
    }

    pub(super) fn resolved_comptime_struct_literal_type(
        &mut self,
        span: Span,
        fields: &[ResolvedComptimeFieldInit],
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
        let Some(expected) = expected else {
            return self.structural_resolved_comptime_struct_literal_type(fields);
        };
        let (def_id, expected_args) = self.expected_nominal_parts(expected)?;
        if self.def_kind_of(def_id) != Some(DefKind::Struct) {
            return None;
        }
        let signature = self.struct_signature_for(def_id)?;
        let field_tys = self.comptime_struct_field_types(&signature, &expected_args)?;
        let field_set = check_required_field_set(
            fields
                .iter()
                .map(|field| NamedField::new(field.span(), field.name())),
            field_tys.keys().map(String::as_str),
        );
        if !field_set.is_valid() {
            return None;
        }
        let mut substitutions = HashMap::new();
        for field in fields {
            let expected_field = *field_tys.get(field.name())?;
            if let Some(actual_field) =
                self.resolved_comptime_struct_field_actual_type(field.value(), expected_field)
            {
                self.probe_type_generic_inference(
                    span,
                    expected_field,
                    actual_field,
                    &mut substitutions,
                )?;
            }
        }
        for field in fields {
            let expected_field =
                self.substitute_current_ty_generics(*field_tys.get(field.name())?, &substitutions)?;
            let actual_field =
                self.resolved_comptime_arg_runtime_type(field.value(), Some(expected_field))?;
            if actual_field != expected_field {
                return None;
            }
        }
        self.substitute_nominal_args(def_id, expected_args, &substitutions)
            .map(ComptimeValueType::Runtime)
    }

    pub(super) fn structural_resolved_comptime_struct_literal_type(
        &mut self,
        fields: &[ResolvedComptimeFieldInit],
    ) -> Option<ComptimeValueType> {
        let mut seen = HashSet::new();
        let mut typed_fields = Vec::with_capacity(fields.len());
        for field in fields {
            if !seen.insert(field.name()) {
                return None;
            }
            typed_fields.push(ComptimeValueFieldType {
                name: field.name().to_string(),
                ty: self.resolved_comptime_expr_type(field.value(), None)?,
            });
        }
        Some(ComptimeValueType::Struct(typed_fields))
    }

    pub(super) fn comptime_nominal_struct_field_type(
        &mut self,
        ty: InternedTyId,
        name: &str,
    ) -> Option<InternedTyId> {
        let (def_id, args) = self.expected_nominal_parts(ty)?;
        if self.def_kind_of(def_id) != Some(DefKind::Struct) {
            return None;
        }
        let signature = self.struct_signature_for(def_id)?;
        self.comptime_struct_field_types(&signature, &args)?
            .get(name)
            .copied()
    }

    pub(super) fn resolved_comptime_struct_field_actual_type(
        &mut self,
        value: &ResolvedComptimeExpr,
        expected: InternedTyId,
    ) -> Option<InternedTyId> {
        self.resolved_comptime_arg_runtime_type(value, Some(expected))
            .filter(|ty| !self.type_contains_generic(*ty))
            .or_else(|| self.resolved_comptime_arg_runtime_type(value, None))
    }

    pub(super) fn substitute_current_ty_generics(
        &mut self,
        ty: InternedTyId,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Option<InternedTyId> {
        let current_module = self.current_execution_module_id();
        let interner = self.working_interners.get_mut(&current_module)?;
        Some(substitute_ty_generics_in_interner(
            interner,
            ty,
            &|generic| substitutions.get(generic).copied(),
        ))
    }

    pub(super) fn expected_nominal_parts(
        &self,
        ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        match self.ty_kind(ty)? {
            TyKind::Nominal { def_id, args, .. } => Some((def_id, args)),
            _ => None,
        }
    }

    pub(super) fn struct_signature_for(
        &self,
        def_id: GlobalDefId,
    ) -> Option<nia_item_signatures::StructSignature> {
        self.signatures_for_module(def_id.module_id)?
            .structs
            .get(&def_id.def_id)
            .cloned()
    }

    pub(super) fn comptime_struct_field_types(
        &mut self,
        signature: &nia_item_signatures::StructSignature,
        expected_args: &[InternedTyId],
    ) -> Option<HashMap<String, InternedTyId>> {
        if signature.generics.len() != expected_args.len() {
            return None;
        }
        let current_module = self.current_execution_module_id();
        let expected_args = expected_args
            .iter()
            .copied()
            .map(|arg| self.import_ty_into_module_or_none(arg, current_module))
            .collect::<Option<Vec<_>>>()?;
        let substitutions = signature
            .generics
            .iter()
            .cloned()
            .zip(expected_args)
            .collect::<HashMap<_, _>>();
        let mut fields = HashMap::new();
        for field in &signature.fields {
            let imported = self.import_ty_into_module_or_none(field.ty, current_module)?;
            let ty = {
                let interner = self.working_interners.get_mut(&current_module)?;
                substitute_ty_generics_in_interner(interner, imported, &|generic| {
                    substitutions.get(generic).copied()
                })
            };
            fields.insert(field.name.clone(), ty);
        }
        Some(fields)
    }

    pub(super) fn substitute_nominal_args(
        &mut self,
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Option<InternedTyId> {
        let current_module = self.current_execution_module_id();
        let args = {
            let interner = self.working_interners.get_mut(&current_module)?;
            args.into_iter()
                .map(|arg| {
                    substitute_ty_generics_in_interner(interner, arg, &|generic| {
                        substitutions.get(generic).copied()
                    })
                })
                .collect()
        };
        self.working_interners
            .get_mut(&current_module)
            .map(|interner| {
                interner.intern(TyKind::Nominal {
                    def_id,
                    args,
                    const_args: Vec::new(),
                })
            })
    }

    pub(super) fn resolved_comptime_switch_expr_type(
        &mut self,
        switch: &ResolvedComptimeSwitch,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
        let target_ty = self.resolved_comptime_arg_runtime_type(switch.target(), None);
        let expected = expected.and_then(|expected| self.usable_comptime_expected_type(expected));
        let mut result_ty = expected.map(ComptimeValueType::Runtime);
        let mut saw_value_arm = false;
        for arm in switch.arms() {
            let arm_ty = result_ty
                .clone()
                .and_then(|expected| {
                    let runtime_expected = expected.runtime();
                    let arm_ty =
                        self.resolved_comptime_switch_arm_type(arm, target_ty, runtime_expected)?;
                    (arm_ty == ComptimeArmType::Value(expected)).then_some(arm_ty)
                })
                .or_else(|| {
                    self.resolved_comptime_switch_arm_type(
                        arm,
                        target_ty,
                        result_ty.as_ref()?.runtime(),
                    )
                })
                .or_else(|| self.resolved_comptime_switch_arm_type(arm, target_ty, None))?;
            let ComptimeArmType::Value(arm_ty) = arm_ty else {
                continue;
            };
            saw_value_arm = true;
            match &result_ty {
                Some(result_ty) if *result_ty != arm_ty => return None,
                Some(_) => {}
                None => result_ty = Some(arm_ty),
            }
        }
        saw_value_arm.then_some(result_ty).flatten()
    }

    pub(super) fn resolved_comptime_switch_arm_type(
        &mut self,
        arm: &nia_comptime_ir::ResolvedComptimeSwitchArm,
        target_ty: Option<InternedTyId>,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeArmType> {
        let target_ty = target_ty?;
        self.check_resolved_comptime_patterns(arm.patterns(), target_ty)?;
        if !self.resolved_comptime_switch_arm_binds_pattern_locals(arm) {
            return self.resolved_comptime_switch_arm_body_type(arm.body(), expected);
        }
        self.push_typed_comptime_scope();
        let result = (|| {
            self.bind_typed_resolved_comptime_patterns(arm.patterns(), target_ty)?;
            self.resolved_comptime_switch_arm_body_type(arm.body(), expected)
        })();
        self.pop_typed_comptime_scope();
        result
    }

    pub(super) fn resolved_comptime_switch_arm_binds_pattern_locals(
        &self,
        arm: &nia_comptime_ir::ResolvedComptimeSwitchArm,
    ) -> bool {
        arm.patterns()
            .iter()
            .any(|pattern| resolved_pattern_local_id(pattern).is_some())
    }

    fn resolved_comptime_switch_has_definite_pattern_mismatch(
        &mut self,
        switch: &ResolvedComptimeSwitch,
    ) -> bool {
        let Some(target_ty) = self.resolved_comptime_arg_runtime_type(switch.target(), None) else {
            return false;
        };
        switch.arms().iter().any(|arm| {
            self.resolved_comptime_patterns_have_definite_mismatch(arm.patterns(), target_ty)
        })
    }

    fn resolved_comptime_patterns_have_definite_mismatch(
        &mut self,
        patterns: &[ResolvedComptimePattern],
        target_ty: InternedTyId,
    ) -> bool {
        patterns
            .iter()
            .any(|pattern| self.resolved_comptime_pattern_has_definite_mismatch(pattern, target_ty))
    }

    fn resolved_comptime_pattern_has_definite_mismatch(
        &mut self,
        pattern: &ResolvedComptimePattern,
        target_ty: InternedTyId,
    ) -> bool {
        match pattern.kind() {
            ResolvedComptimePatternKind::Wildcard { .. }
            | ResolvedComptimePatternKind::Bind { .. } => false,
            ResolvedComptimePatternKind::Pointer { pattern, .. }
            | ResolvedComptimePatternKind::MutPointer { pattern, .. } => {
                let Some(TyKind::Pointer { elem, .. }) = self.ty_kind(target_ty) else {
                    return true;
                };
                self.resolved_comptime_pattern_has_definite_mismatch(pattern, elem)
            }
            ResolvedComptimePatternKind::Expr(expr) => {
                let target_ty = ComptimeValueType::Runtime(target_ty);
                self.resolved_comptime_expr_type(expr, target_ty.runtime())
                    .or_else(|| self.resolved_comptime_expr_type(expr, None))
                    .is_some_and(|pattern_ty| {
                        pattern_ty != target_ty
                            && !self.comptime_equality_types_are_compatible(&target_ty, &pattern_ty)
                    })
            }
            ResolvedComptimePatternKind::Range { start, end, .. } => {
                if !self.is_integer_runtime_type(target_ty) {
                    return true;
                }
                let start_ty = self.resolved_comptime_arg_runtime_type(start, Some(target_ty));
                let end_ty = self.resolved_comptime_arg_runtime_type(end, Some(target_ty));
                matches!(
                    (start_ty, end_ty),
                    (Some(start_ty), Some(end_ty))
                        if start_ty != target_ty || end_ty != target_ty
                )
            }
            ResolvedComptimePatternKind::OptionalSome { pattern, .. } => {
                let Some(TyKind::Optional { elem }) = self.ty_kind(target_ty) else {
                    return true;
                };
                self.resolved_comptime_pattern_has_definite_mismatch(pattern, elem)
            }
            ResolvedComptimePatternKind::OptionalNull { .. } => {
                !matches!(self.ty_kind(target_ty), Some(TyKind::Optional { .. }))
            }
            ResolvedComptimePatternKind::ErrorOk { pattern, .. } => {
                let Some(TyKind::ErrorUnion { value, .. }) = self.ty_kind(target_ty) else {
                    return true;
                };
                self.resolved_comptime_pattern_has_definite_mismatch(pattern, value)
            }
            ResolvedComptimePatternKind::ErrorErr { pattern, .. } => {
                let Some(TyKind::ErrorUnion { error, .. }) = self.ty_kind(target_ty) else {
                    return true;
                };
                self.resolved_comptime_pattern_has_definite_mismatch(pattern, error)
            }
        }
    }

    pub(super) fn check_resolved_comptime_patterns(
        &mut self,
        patterns: &[ResolvedComptimePattern],
        target_ty: InternedTyId,
    ) -> Option<()> {
        for pattern in patterns {
            match pattern.kind() {
                ResolvedComptimePatternKind::Wildcard { .. }
                | ResolvedComptimePatternKind::Bind { .. } => {}
                ResolvedComptimePatternKind::Pointer { pattern, .. }
                | ResolvedComptimePatternKind::MutPointer { pattern, .. } => {
                    let Some(TyKind::Pointer { elem, .. }) = self.ty_kind(target_ty) else {
                        return None;
                    };
                    self.check_resolved_comptime_patterns(std::slice::from_ref(pattern), elem)?;
                }
                ResolvedComptimePatternKind::Expr(expr) => {
                    let target_ty = ComptimeValueType::Runtime(target_ty);
                    let pattern_ty = self
                        .resolved_comptime_expr_type(expr, Some(target_ty.runtime()?))
                        .or_else(|| self.resolved_comptime_expr_type(expr, None))?;
                    if pattern_ty != target_ty
                        && !self.comptime_equality_types_are_compatible(&target_ty, &pattern_ty)
                    {
                        return None;
                    }
                }
                ResolvedComptimePatternKind::Range { start, end, .. } => {
                    if !self.is_integer_runtime_type(target_ty) {
                        return None;
                    }
                    let start_ty =
                        self.resolved_comptime_arg_runtime_type(start, Some(target_ty))?;
                    let end_ty = self.resolved_comptime_arg_runtime_type(end, Some(target_ty))?;
                    if start_ty != target_ty || end_ty != target_ty {
                        return None;
                    }
                }
                ResolvedComptimePatternKind::OptionalSome { pattern, .. } => {
                    let Some(TyKind::Optional { elem }) = self.ty_kind(target_ty) else {
                        return None;
                    };
                    self.check_resolved_comptime_patterns(std::slice::from_ref(pattern), elem)?;
                }
                ResolvedComptimePatternKind::OptionalNull { .. } => {
                    if !matches!(self.ty_kind(target_ty), Some(TyKind::Optional { .. })) {
                        return None;
                    }
                }
                ResolvedComptimePatternKind::ErrorOk { pattern, .. } => {
                    let Some(TyKind::ErrorUnion { value, .. }) = self.ty_kind(target_ty) else {
                        return None;
                    };
                    self.check_resolved_comptime_patterns(std::slice::from_ref(pattern), value)?;
                }
                ResolvedComptimePatternKind::ErrorErr { pattern, .. } => {
                    let Some(TyKind::ErrorUnion { error, .. }) = self.ty_kind(target_ty) else {
                        return None;
                    };
                    self.check_resolved_comptime_patterns(std::slice::from_ref(pattern), error)?;
                }
            }
        }
        Some(())
    }

    pub(super) fn bind_typed_resolved_comptime_patterns(
        &mut self,
        patterns: &[ResolvedComptimePattern],
        target_ty: InternedTyId,
    ) -> Option<()> {
        for pattern in patterns {
            self.bind_typed_resolved_comptime_pattern(pattern, target_ty)?;
        }
        Some(())
    }

    pub(super) fn bind_typed_resolved_comptime_pattern(
        &mut self,
        pattern: &ResolvedComptimePattern,
        target_ty: InternedTyId,
    ) -> Option<()> {
        match pattern.kind() {
            ResolvedComptimePatternKind::Bind { local_id, .. } => {
                self.bind_comptime_local_type(*local_id, ComptimeValueType::Runtime(target_ty));
            }
            ResolvedComptimePatternKind::Pointer { pattern, .. }
            | ResolvedComptimePatternKind::MutPointer { pattern, .. } => {
                let Some(TyKind::Pointer { elem, .. }) = self.ty_kind(target_ty) else {
                    return None;
                };
                self.bind_typed_resolved_comptime_pattern(pattern, elem)?;
            }
            ResolvedComptimePatternKind::OptionalSome { pattern, .. } => {
                let Some(TyKind::Optional { elem }) = self.ty_kind(target_ty) else {
                    return None;
                };
                self.bind_typed_resolved_comptime_pattern(pattern, elem)?;
            }
            ResolvedComptimePatternKind::ErrorOk { pattern, .. } => {
                let Some(TyKind::ErrorUnion { value, .. }) = self.ty_kind(target_ty) else {
                    return None;
                };
                self.bind_typed_resolved_comptime_pattern(pattern, value)?;
            }
            ResolvedComptimePatternKind::ErrorErr { pattern, .. } => {
                let Some(TyKind::ErrorUnion { error, .. }) = self.ty_kind(target_ty) else {
                    return None;
                };
                self.bind_typed_resolved_comptime_pattern(pattern, error)?;
            }
            ResolvedComptimePatternKind::Wildcard { .. }
            | ResolvedComptimePatternKind::OptionalNull { .. }
            | ResolvedComptimePatternKind::Expr(_)
            | ResolvedComptimePatternKind::Range { .. } => {}
        }
        Some(())
    }

    pub(super) fn resolved_comptime_switch_arm_body_type(
        &mut self,
        body: &ResolvedComptimeSwitchArmBody,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeArmType> {
        match body.kind() {
            ResolvedComptimeSwitchArmBodyKind::Expr(expr) => self
                .resolved_comptime_expr_type(expr, expected)
                .map(ComptimeArmType::Value),
            ResolvedComptimeSwitchArmBodyKind::Block(block) => {
                self.resolved_comptime_switch_block_arm_type(block, expected)
            }
            ResolvedComptimeSwitchArmBodyKind::Stmt(stmt) => {
                self.resolved_comptime_stmt_arm_type(stmt, expected)
            }
        }
    }

    pub(super) fn resolved_comptime_switch_block_arm_type(
        &mut self,
        block: &ResolvedComptimeBlock,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeArmType> {
        self.resolved_comptime_block_tail_type(block, expected)
            .map(ComptimeArmType::Value)
    }

    pub(super) fn resolved_comptime_stmt_arm_type(
        &mut self,
        stmt: &nia_comptime_ir::ResolvedComptimeStmt,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeArmType> {
        match stmt.kind() {
            ResolvedComptimeStmtKind::Expr(expr) => self
                .resolved_comptime_expr_type(expr, expected)
                .map(ComptimeArmType::Value),
            ResolvedComptimeStmtKind::Return(_)
            | ResolvedComptimeStmtKind::Break
            | ResolvedComptimeStmtKind::Continue => Some(ComptimeArmType::ControlFlow),
            ResolvedComptimeStmtKind::Binding(_)
            | ResolvedComptimeStmtKind::If { .. }
            | ResolvedComptimeStmtKind::ForIn(_)
            | ResolvedComptimeStmtKind::While { .. }
            | ResolvedComptimeStmtKind::Loop { .. } => None,
        }
    }

    pub(super) fn resolved_comptime_if_expr_type(
        &mut self,
        cond: &ResolvedComptimeExpr,
        then_branch: &ResolvedComptimeBlock,
        else_branch: Option<&ResolvedComptimeExpr>,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
        let bool_ty = self.current_runtime_primitive_type(PrimitiveTy::Bool);
        let cond_ty = self.resolved_comptime_arg_runtime_type(cond, Some(bool_ty))?;
        if cond_ty != bool_ty {
            return None;
        }
        let expected = expected.and_then(|expected| self.usable_comptime_expected_type(expected));
        let else_branch = else_branch?;
        if let Some(expected) = expected {
            let then_ty = self
                .resolved_comptime_block_tail_runtime_type(then_branch, Some(expected))
                .or_else(|| self.resolved_comptime_block_tail_runtime_type(then_branch, None))?;
            let else_ty = self
                .resolved_comptime_arg_runtime_type(else_branch, Some(expected))
                .filter(|else_ty| *else_ty == then_ty)
                .or_else(|| self.resolved_comptime_arg_runtime_type(else_branch, Some(then_ty)))?;
            return (then_ty == else_ty).then_some(ComptimeValueType::Runtime(then_ty));
        }
        let then_ty = self.resolved_comptime_block_tail_type(then_branch, None)?;
        let else_ty = self.resolved_comptime_expr_type(else_branch, None)?;
        (then_ty == else_ty).then_some(then_ty)
    }

    pub(super) fn usable_comptime_expected_type(&self, ty: InternedTyId) -> Option<InternedTyId> {
        match self.ty_kind(ty) {
            Some(TyKind::GenericParam(_)) => None,
            _ => Some(ty),
        }
    }

    pub(super) fn resolved_comptime_block_tail_runtime_type(
        &mut self,
        block: &ResolvedComptimeBlock,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        self.resolved_comptime_block_tail_type(block, expected)
            .and_then(|ty| ty.runtime())
    }

    pub(super) fn resolved_comptime_block_tail_type(
        &mut self,
        block: &ResolvedComptimeBlock,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
        if block.is_empty() {
            return self.resolved_comptime_expr_type(block.tail()?, expected);
        }
        self.push_typed_comptime_scope();
        let result = (|| {
            for stmt in block.stmts() {
                self.bind_typed_resolved_comptime_stmt(stmt)?;
            }
            self.resolved_comptime_expr_type(block.tail()?, expected)
        })();
        self.pop_typed_comptime_scope();
        result
    }

    pub(super) fn bind_typed_resolved_comptime_stmt(
        &mut self,
        stmt: &nia_comptime_ir::ResolvedComptimeStmt,
    ) -> Option<()> {
        match stmt.kind() {
            ResolvedComptimeStmtKind::Binding(binding) => {
                let ty = binding
                    .explicit_type()
                    .map(|ty| self.substitute_ty_generics(ty))
                    .map(ComptimeValueType::Runtime)
                    .or_else(|| self.resolved_comptime_expr_type(binding.value(), None))?;
                self.bind_comptime_local_type(binding.local_id(), ty);
                Some(())
            }
            ResolvedComptimeStmtKind::Expr(_) => Some(()),
            ResolvedComptimeStmtKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.check_resolved_comptime_bool_condition(cond)?;
                self.check_resolved_comptime_block(then_branch)?;
                if let Some(else_branch) = else_branch {
                    self.check_resolved_comptime_block(else_branch)?;
                }
                Some(())
            }
            ResolvedComptimeStmtKind::ForIn(for_in) => {
                self.check_resolved_comptime_for_in_stmt(for_in)
            }
            ResolvedComptimeStmtKind::While { cond, body } => {
                self.check_resolved_comptime_bool_condition(cond)?;
                self.check_resolved_comptime_block(body)
            }
            ResolvedComptimeStmtKind::Loop { body } => self.check_resolved_comptime_block(body),
            ResolvedComptimeStmtKind::Return(_)
            | ResolvedComptimeStmtKind::Break
            | ResolvedComptimeStmtKind::Continue => None,
        }
    }

    pub(super) fn check_resolved_comptime_block(
        &mut self,
        block: &ResolvedComptimeBlock,
    ) -> Option<()> {
        self.push_typed_comptime_scope();
        let result = (|| {
            for stmt in block.stmts() {
                self.check_resolved_comptime_stmt(stmt)?;
            }
            if let Some(tail) = block.tail() {
                self.resolved_comptime_expr_type(tail, None)?;
            }
            Some(())
        })();
        self.pop_typed_comptime_scope();
        result
    }

    pub(super) fn check_resolved_comptime_stmt(
        &mut self,
        stmt: &nia_comptime_ir::ResolvedComptimeStmt,
    ) -> Option<()> {
        match stmt.kind() {
            ResolvedComptimeStmtKind::Binding(_)
            | ResolvedComptimeStmtKind::If { .. }
            | ResolvedComptimeStmtKind::ForIn(_)
            | ResolvedComptimeStmtKind::While { .. }
            | ResolvedComptimeStmtKind::Loop { .. } => self.bind_typed_resolved_comptime_stmt(stmt),
            ResolvedComptimeStmtKind::Expr(_)
            | ResolvedComptimeStmtKind::Break
            | ResolvedComptimeStmtKind::Continue => Some(()),
            ResolvedComptimeStmtKind::Return(Some(expr)) => {
                self.resolved_comptime_expr_type(expr, None)?;
                Some(())
            }
            ResolvedComptimeStmtKind::Return(None) => Some(()),
        }
    }

    pub(super) fn check_resolved_comptime_for_in_stmt(
        &mut self,
        for_in: &nia_comptime_ir::ResolvedComptimeForIn,
    ) -> Option<()> {
        let iter_ty = self.resolved_comptime_expr_type(for_in.iter(), None)?;
        let Some(binding_ty) = self.comptime_for_in_binding_type(iter_ty) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                for_in.iter().span(),
                "comptime for-in expects an Iterator".to_string(),
            ));
            return None;
        };
        self.push_typed_comptime_scope();
        let result = (|| {
            let ComptimeValueType::Runtime(binding_ty) = binding_ty else {
                return None;
            };
            self.bind_typed_resolved_comptime_pattern(for_in.pattern(), binding_ty)?;
            for stmt in for_in.body().stmts() {
                self.check_resolved_comptime_stmt(stmt)?;
            }
            if let Some(tail) = for_in.body().tail() {
                self.resolved_comptime_expr_type(tail, None)?;
            }
            Some(())
        })();
        self.pop_typed_comptime_scope();
        result
    }

    pub(super) fn comptime_for_in_binding_type(
        &mut self,
        iter_ty: ComptimeValueType,
    ) -> Option<ComptimeValueType> {
        let ComptimeValueType::Runtime(iter_ty) = iter_ty else {
            return None;
        };
        let iter_ty =
            self.import_ty_into_module_or_none(iter_ty, self.current_execution_module_id())?;
        if !self.proves_trait_obligation(
            iter_ty,
            TraitId::Builtin(nia_ty::BuiltinTrait::Iterator),
            Vec::new(),
        ) {
            return None;
        }
        let item = self.intern_current_ty(TyKind::Projection {
            self_ty: iter_ty,
            trait_id: TraitId::Builtin(nia_ty::BuiltinTrait::Iterator),
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            name: nia_ty::BuiltinTrait::ITEM_ASSOC_TYPE.to_string(),
        })?;
        Some(ComptimeValueType::Runtime(self.normalize_projection(item)))
    }

    pub(super) fn intern_current_ty(&mut self, kind: TyKind) -> Option<InternedTyId> {
        let module_id = self.current_execution_module_id();
        self.ensure_working_interner(module_id)?;
        self.working_interners
            .get_mut(&module_id)
            .map(|interner| interner.intern(kind))
    }
}
