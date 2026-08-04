use super::ty_substitution::substitute_ty_generics;
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConstTypeCompatibility {
    Mismatch,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConstSliceLenCheck {
    Known(u64),
    Unknown,
    Invalid,
}

impl Analyzer<'_> {
    pub(super) fn substitute_ty_generics(&mut self, ty: InternedTyId) -> InternedTyId {
        let module_id = self.current_execution_module_id();
        let substitutions = self
            .call_locals
            .iter()
            .flat_map(|frame| frame.type_substitutions.iter())
            .map(|(name, ty)| (*name, *ty))
            .collect::<SymbolMap<_>>();
        let interner = self
            .type_contexts
            .get_mut(&module_id)
            .expect("type context must exist for current execution module");
        substitute_ty_generics(interner, ty, &|name| substitutions.get(name).copied())
    }

    pub(super) fn instantiate_resolved_function_generics(
        &mut self,
        span: Span,
        input: ConstFunctionInstantiationInput<'_>,
    ) -> Result<ConstGenericInstantiation, ConstError> {
        let ConstFunctionInstantiationInput {
            signature_module_id,
            signature,
            type_args,
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
        if !type_args.is_empty()
            && let ArityCheck::Mismatch { actual, .. } =
                check_exact_arity(signature.generics.len(), type_args.len())
        {
            return Err(ConstError {
                span,
                message: format!(
                    "generic argument count mismatch for const function: expected {}, got {}",
                    signature.generics.len(),
                    actual
                ),
            });
        }
        let mut substitutions = initial.type_substitutions;
        let mut const_substitutions = initial.const_substitutions;
        if type_args.is_empty() {
            if let Some(expected) = expected_return
                && let Some(expected) = self.type_for_module_or_none(expected, signature_module_id)
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
                let expected =
                    self.const_expected_param_type(signature_module_id, param.ty, &substitutions);
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
                                ResolvedConstExprKind::Switch(_) => {
                                    "const switch expression does not match expected type"
                                }
                                _ => "const call argument does not match expected type",
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
                    let name = self.symbol_name(*generic);
                    return Err(ConstError {
                        span,
                        message: format!("cannot infer const generic type argument `{name}`"),
                    });
                }
            }
        } else {
            for (generic, arg) in signature.generic_params.iter().zip(type_args) {
                match &generic.kind {
                    GenericParamSignatureKind::Type => {
                        let canonical = self.type_for_module(arg.ty(), signature_module_id)?;
                        substitutions.insert(generic.name, canonical);
                    }
                    GenericParamSignatureKind::Const { ty } => {
                        let value = self
                            .const_generic_arg_from_resolved_type_arg(arg, signature_module_id)?;
                        const_substitutions
                            .insert(generic.name, nia_ty::ConstGenericArg { ty: *ty, value });
                    }
                }
            }
        }
        Ok(ConstGenericInstantiation {
            type_substitutions: substitutions,
            const_substitutions,
        })
    }

    fn const_generic_arg_from_resolved_type_arg(
        &mut self,
        arg: &ResolvedConstTypeArg,
        module_id: ModuleId,
    ) -> Result<nia_ty::ConstGenericValue, ConstError> {
        let canonical = self.type_for_module(arg.ty(), module_id)?;
        match self
            .type_contexts
            .get(&module_id)
            .and_then(|interner| interner.get(canonical))
        {
            Some(TyKind::GenericParam(name)) => Ok(nia_ty::ConstGenericValue::GenericParam(*name)),
            _ => Err(ConstError {
                span: arg.span(),
                message: "const generic argument must be a const value".to_string(),
            }),
        }
    }

    pub(super) fn const_expected_param_type(
        &mut self,
        module_id: ModuleId,
        ty: InternedTyId,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> Option<InternedTyId> {
        self.ensure_type_context(module_id)?;
        let types = self.type_contexts.get(&module_id)?;
        Some(substitute_ty_generics(types, ty, &|generic| {
            substitutions.get(generic).copied()
        }))
    }

    pub(super) fn resolved_const_arg_runtime_type(
        &mut self,
        expr: &ResolvedConstExpr,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        self.resolved_const_expr_type(expr, expected)
            .and_then(|ty| ty.runtime())
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
            ResolvedConstExprKind::Switch(switch)
                if self.resolved_const_switch_has_definite_pattern_mismatch(switch) =>
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
        nia_const_eval::eval_resolved_const_int_expr(expr, self)
            .ok()
            .and_then(IntConst::as_i128)
    }

    pub(super) fn probe_resolved_const_array_len_expr(
        &mut self,
        expr: &ResolvedConstExpr,
    ) -> Option<u64> {
        nia_const_eval::eval_resolved_const_array_len_expr(expr, self).ok()
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
                .call_locals
                .iter()
                .rev()
                .find_map(|frame| frame.const_substitutions.get(&name))
                .map(|arg| ConstValueType::Runtime(arg.ty)),
        }
    }

    pub(super) fn resolved_const_expr_type(
        &mut self,
        expr: &ResolvedConstExpr,
        expected: Option<InternedTyId>,
    ) -> Option<ConstValueType> {
        match expr.kind() {
            ResolvedConstExprKind::Name(resolution) => self
                .resolved_const_enum_variant_value_type(expr)
                .or_else(|| self.const_name_resolution_type(resolution.clone())),
            ResolvedConstExprKind::Integer(text) => {
                integer_literal_suffix_ty(text).map(|primitive| {
                    ConstValueType::Runtime(self.current_runtime_primitive_type(primitive))
                })
            }
            ResolvedConstExprKind::Float(text) => {
                let primitive = float_literal_suffix_ty(text).unwrap_or(PrimitiveTy::F64);
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
            ResolvedConstExprKind::ArrayLiteral {
                ty: Some(ty),
                elems,
            } => self.resolved_const_array_literal_type(expr.span(), elems, Some(*ty)),
            ResolvedConstExprKind::StructLiteral {
                ty: Some(ty),
                fields,
            } => self.resolved_const_struct_literal_type(expr.span(), fields, Some(*ty)),
            ResolvedConstExprKind::ArrayLiteral { ty: None, elems } => {
                self.resolved_const_array_literal_type(expr.span(), elems, expected)
            }
            ResolvedConstExprKind::StructLiteral { ty: None, fields } => {
                self.resolved_const_struct_literal_type(expr.span(), fields, expected)
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
            ResolvedConstExprKind::BuiltinMethod { method, lhs } => {
                let lhs_ty = self.resolved_const_expr_type(lhs, None)?;
                self.const_builtin_method_type(*method, lhs_ty)
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
                self.resolved_const_binary_expr_type(lhs, *op, rhs)
            }
            ResolvedConstExprKind::Unary { op, expr: inner } => {
                self.resolved_const_unary_expr_type(*op, inner)
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
            ResolvedConstExprKind::Switch(switch) => {
                self.resolved_const_switch_expr_type(switch, expected)
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
            ResolvedConstExprKind::Call {
                callee,
                type_args,
                args,
            } => {
                if self.resolved_const_enum_variant(callee).is_some() {
                    self.resolved_const_tuple_enum_literal_type(expr.span(), callee, args)
                        .map(ConstValueType::Runtime)
                } else {
                    self.resolved_const_call_return_type(
                        expr.span(),
                        callee,
                        type_args,
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
                    self.current_runtime_primitive_type(PrimitiveTy::Void),
                ))
            }
        }
    }

    fn check_resolved_const_assignment(&mut self, span: Span, assign: &ResolvedConstAssign) {
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
            ConstValueType::Struct(_)
            | ConstValueType::Int
            | ConstValueType::Bool
            | ConstValueType::String => None,
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
        switch: &ResolvedConstSwitch,
        local_id: LocalId,
    ) -> Option<InternedTyId> {
        let target_ty = self.resolved_const_arg_runtime_type(switch.target(), None)?;
        for arm in switch.arms() {
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

    fn resolved_const_enum_pattern_fields<'a>(
        &mut self,
        variant_expr: &ResolvedConstExpr,
        fields: &'a ConstEnumPatternFields<ResolvedConstPattern>,
        target_ty: InternedTyId,
    ) -> Option<Vec<(&'a ResolvedConstPattern, InternedTyId)>> {
        let (enum_id, variant) = self.resolved_const_enum_variant(variant_expr)?;
        let (target_enum, target_args) = self.expected_nominal_parts(target_ty)?;
        if target_enum != enum_id || !target_args.is_empty() {
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
                ConstEnumPatternFields::Named(patterns),
                nia_item_signatures::EnumVariantPayloadSignature::Named(expected),
            ) => {
                let field_set = check_required_field_set(
                    patterns
                        .iter()
                        .map(|field| NamedField::new(field.span, field.name)),
                    expected.iter().map(|field| field.name),
                );
                if !field_set.is_valid() {
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
            ResolvedConstPatternKind::EnumVariant {
                variant, fields, ..
            } => self
                .resolved_const_enum_pattern_fields(variant, fields, target_ty)?
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
            ConstValueType::Struct(_) => lhs.structural_field(name).cloned(),
            ConstValueType::Runtime(ty) => self
                .const_nominal_struct_field_type(*ty, name)
                .map(ConstValueType::Runtime),
            ConstValueType::Array { .. }
            | ConstValueType::Int
            | ConstValueType::Bool
            | ConstValueType::String => None,
        }
    }

    pub(super) fn const_builtin_method_type(
        &mut self,
        method: BuiltinTraitMethod,
        lhs: ConstValueType,
    ) -> Option<ConstValueType> {
        let receiver_ty = match lhs {
            ConstValueType::Array { .. } if method == BuiltinTraitMethod::Len => {
                return Some(ConstValueType::Runtime(
                    self.current_runtime_primitive_type(PrimitiveTy::Usize),
                ));
            }
            ConstValueType::Runtime(ty) => ty,
            ConstValueType::Array { .. }
            | ConstValueType::Struct(_)
            | ConstValueType::Int
            | ConstValueType::Bool
            | ConstValueType::String => return None,
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
            BuiltinTraitMethod::Len => Some(ConstValueType::Runtime(
                self.current_runtime_primitive_type(PrimitiveTy::Usize),
            )),
            BuiltinTraitMethod::Start | BuiltinTraitMethod::End => self
                .resolve_associated_type_projection(
                    receiver_ty,
                    TraitId::Builtin(trait_id),
                    &[],
                    &[],
                    &nia_symbol::known::OUTPUT,
                )
                .and_then(|ty| self.type_for_module_or_none(ty, self.current_execution_module_id()))
                .map(ConstValueType::Runtime),
            _ => None,
        }
    }

    pub(super) fn resolved_const_index_type(
        &mut self,
        span: Span,
        lhs: ConstValueType,
        index: &ResolvedConstExpr,
    ) -> Option<ConstValueType> {
        match lhs {
            ConstValueType::Array { .. } => {
                self.check_resolved_const_array_index_operand(index);
                let (elem, len) = lhs.array_elem()?;
                self.check_resolved_const_index_bounds(span, index, len);
                Some(elem.clone())
            }
            ConstValueType::Runtime(ty) => self.resolved_const_runtime_index_type(span, ty, index),
            ConstValueType::Struct(_)
            | ConstValueType::Int
            | ConstValueType::Bool
            | ConstValueType::String => {
                self.visit_resolved_const_index_operand(index);
                self.push_const_type_error(span, "const index target is not an array or slice");
                None
            }
        }
    }

    pub(super) fn resolved_const_runtime_index_type(
        &mut self,
        span: Span,
        lhs: InternedTyId,
        index: &ResolvedConstExpr,
    ) -> Option<ConstValueType> {
        let (len, elem) = match self.ty_kind(lhs) {
            Some(TyKind::Array { len, elem }) => (Some(len), elem),
            Some(TyKind::Slice { elem, .. }) => (None, elem),
            Some(TyKind::Primitive(_)) => {
                self.visit_resolved_const_index_operand(index);
                self.push_const_type_error(span, "const index target is not an array or slice");
                return None;
            }
            _ => {
                self.visit_resolved_const_index_operand(index);
                return None;
            }
        };
        self.check_resolved_const_array_index_operand(index);
        let len = len.and_then(|len| self.array_len_const_value(len));
        self.check_resolved_const_index_bounds(span, index, len);
        self.type_for_module_or_none(elem, self.current_execution_module_id())
            .map(ConstValueType::Runtime)
    }

    fn visit_resolved_const_index_operand(&mut self, index: &ResolvedConstExpr) {
        let _ = self.resolved_const_arg_runtime_type(index, None);
    }

    fn check_resolved_const_array_index_operand(&mut self, index: &ResolvedConstExpr) {
        let index_ty = self.resolved_const_arg_runtime_type(index, None);
        if index_ty.is_some_and(|index_ty| {
            self.const_runtime_type_is_known(index_ty) && !self.is_integer_runtime_type(index_ty)
        }) {
            self.push_const_type_error(index.span(), "const array index must have an integer type");
        }
    }

    fn check_resolved_const_index_bounds(
        &mut self,
        span: Span,
        index: &ResolvedConstExpr,
        len: Option<u64>,
    ) {
        let Some(index_value) = self.probe_resolved_const_int_expr(index) else {
            return;
        };
        let Ok(index_value) = u64::try_from(index_value) else {
            self.push_const_type_error(
                index.span(),
                "const array index must be a non-negative array length",
            );
            return;
        };
        if len.is_some_and(|len| index_value >= len) {
            self.push_const_type_error(
                span,
                &format!("const array index {index_value} is out of bounds"),
            );
        }
    }

    pub(super) fn resolved_const_slice_type(
        &mut self,
        span: Span,
        lhs: ConstValueType,
        range: &nia_const_ir::ResolvedConstSliceRange,
        expected: Option<InternedTyId>,
    ) -> Option<ConstValueType> {
        match lhs {
            ConstValueType::Array { .. } => {
                self.check_resolved_const_array_slice_bound_types(range);
                let (elem, len) = lhs.array_elem()?;
                let expected_len = self.expected_const_array_len(expected);
                match self.resolved_const_slice_len(span, len, expected_len, range) {
                    ConstSliceLenCheck::Known(actual_len) => {
                        self.const_slice_result_type(span, elem.clone(), actual_len, expected)
                    }
                    ConstSliceLenCheck::Unknown => {
                        self.const_unknown_slice_result_type(elem.clone(), expected)
                    }
                    ConstSliceLenCheck::Invalid => None,
                }
            }
            ConstValueType::Runtime(ty) => {
                self.resolved_const_runtime_slice_type(span, ty, range, expected)
            }
            ConstValueType::Struct(_)
            | ConstValueType::Int
            | ConstValueType::Bool
            | ConstValueType::String => {
                self.visit_resolved_const_slice_bounds(range);
                self.push_const_type_error(span, "const slice target is not an array or slice");
                None
            }
        }
    }

    pub(super) fn resolved_const_runtime_slice_type(
        &mut self,
        span: Span,
        lhs: InternedTyId,
        range: &nia_const_ir::ResolvedConstSliceRange,
        expected: Option<InternedTyId>,
    ) -> Option<ConstValueType> {
        let (len, elem) = match self.ty_kind(lhs) {
            Some(TyKind::Array { len, elem }) => (Some(len), elem),
            Some(TyKind::Slice { elem, .. }) => (None, elem),
            Some(TyKind::Primitive(_)) => {
                self.visit_resolved_const_slice_bounds(range);
                self.push_const_type_error(span, "const slice target is not an array or slice");
                return None;
            }
            _ => {
                self.visit_resolved_const_slice_bounds(range);
                return None;
            }
        };
        self.check_resolved_const_array_slice_bound_types(range);
        let known_len = len.and_then(|len| self.array_len_const_value(len));
        let expected_len = self.expected_const_array_len(expected);
        let elem = self.type_for_module_or_none(elem, self.current_execution_module_id())?;
        let elem = ConstValueType::Runtime(elem);
        match self.resolved_const_slice_len(span, known_len, expected_len, range) {
            ConstSliceLenCheck::Known(actual_len) => {
                self.const_slice_result_type(span, elem, actual_len, expected)
            }
            ConstSliceLenCheck::Unknown => self.const_unknown_slice_result_type(elem, expected),
            ConstSliceLenCheck::Invalid => None,
        }
    }

    pub(super) fn const_slice_result_type(
        &mut self,
        span: Span,
        elem: ConstValueType,
        actual_len: u64,
        expected: Option<InternedTyId>,
    ) -> Option<ConstValueType> {
        if let Some((expected_len, expected_elem)) =
            expected.and_then(|expected| self.expected_array_parts(expected))
            && elem.runtime() == Some(expected_elem)
        {
            let expected_value = self.array_len_const_value(expected_len.clone());
            let len =
                match check_array_literal_len(Some(expected_len), expected_value, Some(actual_len))
                {
                    ArrayLiteralLenCheck::Accepted(len) => len,
                    ArrayLiteralLenCheck::Mismatch { expected, actual } => {
                        self.push_const_type_error(
                            span,
                            &format!(
                                "const slice length mismatch: expected {expected}, got {actual}"
                            ),
                        );
                        return None;
                    }
                    ArrayLiteralLenCheck::Unknown => return None,
                };
            return self
                .const_runtime_type(
                    expected_elem,
                    |elem| TyKind::Array { len, elem },
                    self.current_execution_module_id(),
                )
                .map(ConstValueType::Runtime);
        }
        Some(ConstValueType::Array {
            elem: Box::new(elem),
            len: Some(actual_len),
        })
    }

    fn const_unknown_slice_result_type(
        &mut self,
        elem: ConstValueType,
        expected: Option<InternedTyId>,
    ) -> Option<ConstValueType> {
        if let Some((_, expected_elem)) =
            expected.and_then(|expected| self.expected_array_parts(expected))
            && elem.runtime() == Some(expected_elem)
        {
            return expected.map(ConstValueType::Runtime);
        }
        Some(ConstValueType::Array {
            elem: Box::new(elem),
            len: None,
        })
    }

    fn visit_resolved_const_slice_bounds(&mut self, range: &nia_const_ir::ResolvedConstSliceRange) {
        for bound in [range.start(), range.end()].into_iter().flatten() {
            let _ = self.resolved_const_arg_runtime_type(bound, None);
        }
    }

    fn check_resolved_const_array_slice_bound_types(
        &mut self,
        range: &nia_const_ir::ResolvedConstSliceRange,
    ) {
        for (bound, context) in [(range.start(), "start"), (range.end(), "end")] {
            let Some(bound) = bound else {
                continue;
            };
            let bound_ty = self.resolved_const_arg_runtime_type(bound, None);
            if bound_ty.is_some_and(|bound_ty| {
                self.const_runtime_type_is_known(bound_ty)
                    && !self.is_integer_runtime_type(bound_ty)
            }) {
                self.push_const_type_error(
                    bound.span(),
                    &format!("const slice range {context} must have an integer type"),
                );
            }
        }
    }

    fn resolved_const_slice_len(
        &mut self,
        span: Span,
        source_len: Option<u64>,
        expected_len: Option<u64>,
        range: &nia_const_ir::ResolvedConstSliceRange,
    ) -> ConstSliceLenCheck {
        let start = range.start().map_or(ConstSliceLenCheck::Known(0), |start| {
            self.resolved_const_slice_bound(start, "start")
        });
        let explicit_end = range
            .end()
            .map(|end| self.resolved_const_slice_bound(end, "end"));
        if start == ConstSliceLenCheck::Invalid || explicit_end == Some(ConstSliceLenCheck::Invalid)
        {
            return ConstSliceLenCheck::Invalid;
        }
        let ConstSliceLenCheck::Known(start) = start else {
            return ConstSliceLenCheck::Unknown;
        };
        let end = match explicit_end {
            Some(ConstSliceLenCheck::Known(end)) => Some(end),
            Some(ConstSliceLenCheck::Unknown) => None,
            Some(ConstSliceLenCheck::Invalid) => unreachable!(),
            None => source_len.or_else(|| expected_len.and_then(|len| start.checked_add(len))),
        };
        let Some(mut end) = end else {
            return ConstSliceLenCheck::Unknown;
        };
        if range.is_inclusive()
            && let Some(inclusive_end) = end.checked_add(1)
        {
            end = inclusive_end;
        } else if range.is_inclusive() {
            self.push_const_type_error(span, "const slice inclusive end is too large");
            return ConstSliceLenCheck::Invalid;
        }
        if start > end {
            self.push_const_type_error(
                span,
                &format!("const slice range {start}..{end} is out of bounds"),
            );
            return ConstSliceLenCheck::Invalid;
        }
        if let Some(source_len) = source_len
            && end > source_len
        {
            self.push_const_type_error(
                span,
                &format!("const slice range {start}..{end} is out of bounds"),
            );
            return ConstSliceLenCheck::Invalid;
        }
        ConstSliceLenCheck::Known(end - start)
    }

    fn resolved_const_slice_bound(
        &mut self,
        bound: &ResolvedConstExpr,
        context: &str,
    ) -> ConstSliceLenCheck {
        let Some(value) = self.probe_resolved_const_int_expr(bound) else {
            return ConstSliceLenCheck::Unknown;
        };
        match u64::try_from(value) {
            Ok(value) => ConstSliceLenCheck::Known(value),
            Err(_) => {
                self.push_const_type_error(
                    bound.span(),
                    &format!("const slice range {context} must be a non-negative array length"),
                );
                ConstSliceLenCheck::Invalid
            }
        }
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
            ConstValueType::Struct(fields) => fields
                .into_iter()
                .map(|field| {
                    Some(ConstValueFieldType {
                        name: field.name,
                        ty: self.value_type_for_module(field.ty, target_module_id)?,
                    })
                })
                .collect::<Option<Vec<_>>>()
                .map(ConstValueType::Struct),
            ConstValueType::Int => Some(ConstValueType::Int),
            ConstValueType::Bool => Some(ConstValueType::Bool),
            ConstValueType::String => Some(ConstValueType::String),
        }
    }

    pub(super) fn resolved_const_array_literal_type(
        &mut self,
        span: Span,
        elems: &ResolvedConstArrayElements,
        expected: Option<InternedTyId>,
    ) -> Option<ConstValueType> {
        let expected_parts = expected.and_then(|expected| self.expected_array_parts(expected));
        if expected_parts.is_none() {
            let inferred = self.structural_resolved_const_array_literal_type(span, elems);
            if expected.is_some_and(|expected| {
                self.const_runtime_type_is_known(expected)
                    && !matches!(self.ty_kind(expected), Some(TyKind::ConstOnly))
            }) {
                self.push_const_type_error(
                    span,
                    "const array literal expected type is not an array",
                );
                return None;
            }
            return inferred;
        }
        let mut types_match = true;
        let (elem_ty, actual_len) = match elems.kind() {
            ResolvedConstArrayElementsKind::List(elems) => {
                let expected_elem = expected_parts.as_ref().map(|(_, elem)| *elem)?;
                let elem_ty = self.resolved_const_array_list_elem_type(
                    elems,
                    expected_elem,
                    &mut types_match,
                );
                (elem_ty, Some(elems.len() as u64))
            }
            ResolvedConstArrayElementsKind::Repeat { value, count } => {
                let expected_elem = expected_parts.as_ref().map(|(_, elem)| *elem);
                let elem_ty = self.resolved_const_arg_runtime_type(value, expected_elem);
                if let (Some(expected), Some(actual)) = (expected_elem, elem_ty)
                    && !self.const_function_types_match(expected, actual)
                {
                    if self.const_runtime_type_is_known(expected)
                        && self.const_runtime_type_is_known(actual)
                    {
                        self.push_const_type_error(
                            value.span(),
                            "const array literal element does not match its expected type",
                        );
                    }
                    types_match = false;
                }
                let count_ty = self.resolved_const_arg_runtime_type(count, None);
                if count_ty.is_some_and(|count_ty| {
                    self.const_runtime_type_is_known(count_ty)
                        && !self.is_integer_runtime_type(count_ty)
                }) {
                    self.push_const_type_error(
                        count.span(),
                        "const array repeat count must have an integer type",
                    );
                    types_match = false;
                }
                let actual_len = self.probe_resolved_const_array_len_expr(count);
                let elem_ty = elem_ty.or(expected_elem)?;
                (elem_ty, actual_len)
            }
        };
        let expected_len = expected_parts.as_ref().map(|(len, _)| len.clone());
        let expected_value = expected_len
            .clone()
            .and_then(|len| self.array_len_const_value(len));
        let len = match check_array_literal_len(expected_len, expected_value, actual_len) {
            ArrayLiteralLenCheck::Accepted(len) => len,
            ArrayLiteralLenCheck::Mismatch { expected, actual } => {
                self.push_const_type_error(
                    span,
                    &format!(
                        "const array literal length mismatch: expected {expected}, got {actual}"
                    ),
                );
                return None;
            }
            ArrayLiteralLenCheck::Unknown => return None,
        };
        if !types_match {
            return None;
        }
        self.const_runtime_type(
            elem_ty,
            |elem| TyKind::Array { len, elem },
            self.current_execution_module_id(),
        )
        .map(ConstValueType::Runtime)
    }

    pub(super) fn structural_resolved_const_array_literal_type(
        &mut self,
        span: Span,
        elems: &ResolvedConstArrayElements,
    ) -> Option<ConstValueType> {
        let (elem_ty, len) = match elems.kind() {
            ResolvedConstArrayElementsKind::List(elems) => {
                if elems.is_empty() {
                    self.push_const_type_error(
                        span,
                        "empty const array literal requires an element type",
                    );
                    return None;
                }
                let mut elem_ty = None;
                let mut types_match = true;
                for elem in elems {
                    let Some(actual) = self.resolved_const_expr_type(elem, None) else {
                        types_match = false;
                        continue;
                    };
                    match &elem_ty {
                        Some(expected) if *expected != actual => {
                            if self.const_value_types_have_known_mismatch(expected, &actual) {
                                self.push_const_type_error(
                                    elem.span(),
                                    "const array literal has incompatible element types",
                                );
                            }
                            types_match = false;
                        }
                        Some(_) => {}
                        None => elem_ty = Some(actual),
                    }
                }
                if !types_match {
                    return None;
                }
                (elem_ty?, Some(elems.len() as u64))
            }
            ResolvedConstArrayElementsKind::Repeat { value, count } => {
                let elem_ty = self.resolved_const_expr_type(value, None);
                let count_ty = self.resolved_const_arg_runtime_type(count, None);
                if count_ty.is_some_and(|count_ty| {
                    self.const_runtime_type_is_known(count_ty)
                        && !self.is_integer_runtime_type(count_ty)
                }) {
                    self.push_const_type_error(
                        count.span(),
                        "const array repeat count must have an integer type",
                    );
                    return None;
                }
                let len = self.probe_resolved_const_array_len_expr(count);
                let elem_ty = elem_ty?;
                (elem_ty, len)
            }
        };
        Some(ConstValueType::Array {
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

    pub(super) fn resolved_const_array_list_elem_type(
        &mut self,
        elems: &[ResolvedConstExpr],
        elem_ty: InternedTyId,
        types_match: &mut bool,
    ) -> InternedTyId {
        for elem in elems {
            let Some(actual) = self.resolved_const_arg_runtime_type(elem, Some(elem_ty)) else {
                continue;
            };
            if !self.const_function_types_match(elem_ty, actual) {
                if self.const_runtime_type_is_known(elem_ty)
                    && self.const_runtime_type_is_known(actual)
                {
                    self.push_const_type_error(
                        elem.span(),
                        "const array literal element does not match its expected type",
                    );
                }
                *types_match = false;
            }
        }
        elem_ty
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
                | TyKind::ConstOnly
                | TyKind::Primitive(_)
                | TyKind::BuiltinType(_)
                | TyKind::Vector { .. },
            )
            | None => false,
        }
    }

    pub(super) fn resolved_const_struct_literal_type(
        &mut self,
        span: Span,
        fields: &[ResolvedConstFieldInit],
        expected: Option<InternedTyId>,
    ) -> Option<ConstValueType> {
        let Some(expected) = expected else {
            return self.structural_resolved_const_struct_literal_type(fields);
        };
        let Some((def_id, expected_args)) = self.expected_nominal_parts(expected) else {
            let _ = self.structural_resolved_const_struct_literal_type(fields);
            if self.const_runtime_type_is_known(expected)
                && !matches!(self.ty_kind(expected), Some(TyKind::ConstOnly))
            {
                self.push_const_type_error(
                    span,
                    "const struct literal expected type is not a struct",
                );
            }
            return None;
        };
        if self.def_kind_of(def_id) != Some(DefKind::Struct) {
            let _ = self.structural_resolved_const_struct_literal_type(fields);
            return None;
        }
        let signature = self.struct_signature_for(def_id)?;
        let field_tys = self.const_struct_field_types(&signature, &expected_args)?;
        let field_set = check_required_field_set(
            fields
                .iter()
                .map(|field| NamedField::new(field.span(), *field.name_symbol())),
            signature.fields.iter().map(|field| field.name),
        );
        let fields_are_valid = field_set.is_valid();
        for field in &field_set.duplicate_fields {
            let name = self.symbol_name(field.name);
            self.push_const_type_error(
                field.span,
                &format!("duplicate const struct field `{name}`"),
            );
        }
        for field in &field_set.unknown_fields {
            let name = self.symbol_name(field.name);
            self.push_const_type_error(field.span, &format!("unknown const struct field `{name}`"));
        }
        for name in &field_set.missing_fields {
            let name = self.symbol_name(*name);
            self.push_const_type_error(span, &format!("missing const struct field `{name}`"));
        }
        let mut substitutions = SymbolMap::default();
        let mut actual_fields = Vec::with_capacity(fields.len());
        for field in fields {
            let Some(expected_field) = field_tys.get(field.name_symbol()).copied() else {
                let _ = self.resolved_const_expr_type(field.value(), None);
                actual_fields.push((None, false));
                continue;
            };
            let diagnostics_before_field = self.diagnostics.len();
            let actual_field =
                self.resolved_const_contextual_value_type(field.value(), expected_field);
            if let Some(actual_field) = actual_field {
                let _ = self.probe_type_generic_inference(
                    span,
                    expected_field,
                    actual_field,
                    &mut substitutions,
                );
            }
            actual_fields.push((
                actual_field,
                self.diagnostics.len() != diagnostics_before_field,
            ));
        }

        let mut types_match = true;
        for (field, (actual_field, field_has_diagnostic)) in fields.iter().zip(actual_fields) {
            let Some(raw_expected_field) = field_tys.get(field.name_symbol()).copied() else {
                continue;
            };
            let expected_field =
                self.substitute_current_ty_generics(raw_expected_field, &substitutions)?;
            let actual_field = actual_field
                .filter(|actual| !self.type_contains_generic(*actual))
                .or_else(|| {
                    if field_has_diagnostic {
                        None
                    } else {
                        self.resolved_const_arg_runtime_type(field.value(), Some(expected_field))
                    }
                });
            let Some(actual_field) = actual_field else {
                continue;
            };
            if !self.const_function_types_match(expected_field, actual_field) {
                if self.const_runtime_type_is_known(expected_field)
                    && self.const_runtime_type_is_known(actual_field)
                {
                    self.push_const_type_error(
                        field.value().span(),
                        "const struct literal field does not match its expected type",
                    );
                }
                types_match = false;
            }
        }
        if !fields_are_valid || !types_match {
            return None;
        }
        self.substitute_nominal_args(def_id, expected_args, &substitutions)
            .map(ConstValueType::Runtime)
    }

    pub(super) fn structural_resolved_const_struct_literal_type(
        &mut self,
        fields: &[ResolvedConstFieldInit],
    ) -> Option<ConstValueType> {
        let mut seen = HashSet::new();
        let mut typed_fields = Vec::with_capacity(fields.len());
        let mut fields_are_valid = true;
        for field in fields {
            let is_first = seen.insert(*field.name_symbol());
            if !is_first {
                let name = self.symbol_name(*field.name_symbol());
                self.push_const_type_error(
                    field.span(),
                    &format!("duplicate const struct field `{name}`"),
                );
                fields_are_valid = false;
            }
            let Some(ty) = self.resolved_const_expr_type(field.value(), None) else {
                fields_are_valid = false;
                continue;
            };
            if is_first {
                typed_fields.push(ConstValueFieldType {
                    name: *field.name_symbol(),
                    ty,
                });
            }
        }
        fields_are_valid.then_some(ConstValueType::Struct(typed_fields))
    }

    pub(super) fn const_nominal_struct_field_type(
        &mut self,
        ty: InternedTyId,
        name: &SymbolId,
    ) -> Option<InternedTyId> {
        let (def_id, args) = self.expected_nominal_parts(ty)?;
        if self.def_kind_of(def_id) != Some(DefKind::Struct) {
            return None;
        }
        let signature = self.struct_signature_for(def_id)?;
        self.const_struct_field_types(&signature, &args)?
            .get(name)
            .copied()
    }

    pub(super) fn resolved_const_contextual_value_type(
        &mut self,
        value: &ResolvedConstExpr,
        expected: InternedTyId,
    ) -> Option<InternedTyId> {
        self.resolved_const_arg_runtime_type(value, Some(expected))
            .filter(|ty| !self.type_contains_generic(*ty))
            .or_else(|| self.resolved_const_arg_runtime_type(value, None))
    }

    pub(super) fn substitute_current_ty_generics(
        &mut self,
        ty: InternedTyId,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> Option<InternedTyId> {
        let current_module = self.current_execution_module_id();
        let types = self.type_contexts.get(&current_module)?;
        Some(substitute_ty_generics(types, ty, &|generic| {
            substitutions.get(generic).copied()
        }))
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
            .as_ref()
            .structs
            .get(&def_id.def_id)
            .cloned()
    }

    pub(super) fn const_struct_field_types(
        &mut self,
        signature: &nia_item_signatures::StructSignature,
        expected_args: &[InternedTyId],
    ) -> Option<SymbolMap<InternedTyId>> {
        if signature.generics.len() != expected_args.len() {
            return None;
        }
        let current_module = self.current_execution_module_id();
        let expected_args = expected_args
            .iter()
            .copied()
            .map(|arg| self.type_for_module_or_none(arg, current_module))
            .collect::<Option<Vec<_>>>()?;
        let substitutions = signature
            .generics
            .iter()
            .cloned()
            .zip(expected_args)
            .collect::<SymbolMap<_>>();
        let mut fields = SymbolMap::default();
        for field in &signature.fields {
            let canonical = self.type_for_module_or_none(field.ty, current_module)?;
            let ty = {
                let types = self.type_contexts.get(&current_module)?;
                substitute_ty_generics(types, canonical, &|generic| {
                    substitutions.get(generic).copied()
                })
            };
            fields.insert(field.name, ty);
        }
        Some(fields)
    }

    pub(super) fn substitute_nominal_args(
        &mut self,
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> Option<InternedTyId> {
        let current_module = self.current_execution_module_id();
        let args = {
            let types = self.type_contexts.get(&current_module)?;
            args.into_iter()
                .map(|arg| {
                    substitute_ty_generics(types, arg, &|generic| {
                        substitutions.get(generic).copied()
                    })
                })
                .collect()
        };
        self.type_contexts.get(&current_module).map(|types| {
            types.intern(TyKind::Nominal {
                def_id,
                args,
                const_args: Vec::new(),
            })
        })
    }

    pub(super) fn resolved_const_switch_expr_type(
        &mut self,
        switch: &ResolvedConstSwitch,
        expected: Option<InternedTyId>,
    ) -> Option<ConstValueType> {
        let target_ty = self.resolved_const_arg_runtime_type(switch.target(), None);
        let expected = expected.and_then(|expected| self.usable_const_expected_type(expected));
        let mut result_ty = expected.map(ConstValueType::Runtime);
        let mut saw_value_arm = false;
        let mut all_arms_typed = true;
        for arm in switch.arms() {
            if target_ty.is_some_and(|target_ty| {
                self.const_runtime_type_is_known(target_ty)
                    && self
                        .resolved_const_patterns_have_definite_mismatch(arm.patterns(), target_ty)
            }) {
                self.push_const_type_error(
                    arm.span(),
                    "const switch pattern does not match the target type",
                );
                let _ = self.resolved_const_switch_arm_body_type(
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
                        self.resolved_const_switch_arm_type(arm, target_ty, runtime_expected)?;
                    (arm_ty == ConstArmType::Value(expected)).then_some(arm_ty)
                })
                .or_else(|| {
                    self.resolved_const_switch_arm_type(
                        arm,
                        target_ty,
                        result_ty.as_ref()?.runtime(),
                    )
                })
                .or_else(|| self.resolved_const_switch_arm_type(arm, target_ty, None));
            let Some(arm_ty) = arm_ty else {
                if self.diagnostics.len() == diagnostics_before_arm {
                    let _ = self.resolved_const_switch_arm_body_type(
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
                            "const switch arms have incompatible result types",
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
        saw_value_arm.then_some(result_ty).flatten()
    }

    fn const_value_types_have_known_mismatch(
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

    pub(super) fn resolved_const_switch_arm_type(
        &mut self,
        arm: &nia_const_ir::ResolvedConstSwitchArm,
        target_ty: Option<InternedTyId>,
        expected: Option<InternedTyId>,
    ) -> Option<ConstArmType> {
        let target_ty = target_ty?;
        self.check_resolved_const_patterns(arm.patterns(), target_ty)?;
        if !self.resolved_const_switch_arm_binds_pattern_locals(arm) {
            return self.resolved_const_switch_arm_body_type(arm.body(), expected);
        }
        self.push_typed_const_scope();
        let result = (|| {
            self.bind_typed_resolved_const_patterns(arm.patterns(), target_ty)?;
            self.resolved_const_switch_arm_body_type(arm.body(), expected)
        })();
        self.pop_typed_const_scope();
        result
    }

    pub(super) fn resolved_const_switch_arm_binds_pattern_locals(
        &self,
        arm: &nia_const_ir::ResolvedConstSwitchArm,
    ) -> bool {
        arm.patterns()
            .iter()
            .any(|pattern| resolved_pattern_local_id(pattern).is_some())
    }

    fn resolved_const_switch_has_definite_pattern_mismatch(
        &mut self,
        switch: &ResolvedConstSwitch,
    ) -> bool {
        let Some(target_ty) = self.resolved_const_arg_runtime_type(switch.target(), None) else {
            return false;
        };
        switch.arms().iter().any(|arm| {
            self.resolved_const_patterns_have_definite_mismatch(arm.patterns(), target_ty)
        })
    }

    fn resolved_const_patterns_have_definite_mismatch(
        &mut self,
        patterns: &[ResolvedConstPattern],
        target_ty: InternedTyId,
    ) -> bool {
        patterns
            .iter()
            .any(|pattern| self.resolved_const_pattern_has_definite_mismatch(pattern, target_ty))
    }

    fn resolved_const_pattern_has_definite_mismatch(
        &mut self,
        pattern: &ResolvedConstPattern,
        target_ty: InternedTyId,
    ) -> bool {
        match pattern.kind() {
            ResolvedConstPatternKind::Wildcard { .. } | ResolvedConstPatternKind::Bind { .. } => {
                false
            }
            ResolvedConstPatternKind::Pointer { pattern, .. }
            | ResolvedConstPatternKind::MutPointer { pattern, .. } => {
                let Some(TyKind::Pointer { elem, .. }) = self.ty_kind(target_ty) else {
                    return true;
                };
                self.resolved_const_pattern_has_definite_mismatch(pattern, elem)
            }
            ResolvedConstPatternKind::Expr(expr) => {
                let target_ty = ConstValueType::Runtime(target_ty);
                self.resolved_const_expr_type(expr, target_ty.runtime())
                    .or_else(|| self.resolved_const_expr_type(expr, None))
                    .is_some_and(|pattern_ty| {
                        pattern_ty != target_ty
                            && !self.const_equality_types_are_compatible(&target_ty, &pattern_ty)
                    })
            }
            ResolvedConstPatternKind::Range { start, end, .. } => {
                if !self.is_integer_runtime_type(target_ty) {
                    return true;
                }
                let start_ty = self.resolved_const_arg_runtime_type(start, Some(target_ty));
                let end_ty = self.resolved_const_arg_runtime_type(end, Some(target_ty));
                matches!(
                    (start_ty, end_ty),
                    (Some(start_ty), Some(end_ty))
                        if start_ty != target_ty || end_ty != target_ty
                )
            }
            ResolvedConstPatternKind::OptionalSome { pattern, .. } => {
                let Some(TyKind::Optional { elem }) = self.ty_kind(target_ty) else {
                    return true;
                };
                self.resolved_const_pattern_has_definite_mismatch(pattern, elem)
            }
            ResolvedConstPatternKind::OptionalNull { .. } => {
                !matches!(self.ty_kind(target_ty), Some(TyKind::Optional { .. }))
            }
            ResolvedConstPatternKind::ErrorOk { pattern, .. } => {
                let Some(TyKind::ErrorUnion { value, .. }) = self.ty_kind(target_ty) else {
                    return true;
                };
                self.resolved_const_pattern_has_definite_mismatch(pattern, value)
            }
            ResolvedConstPatternKind::ErrorErr { pattern, .. } => {
                let Some(TyKind::ErrorUnion { error, .. }) = self.ty_kind(target_ty) else {
                    return true;
                };
                self.resolved_const_pattern_has_definite_mismatch(pattern, error)
            }
            ResolvedConstPatternKind::EnumVariant {
                variant, fields, ..
            } => {
                let Some(fields) =
                    self.resolved_const_enum_pattern_fields(variant, fields, target_ty)
                else {
                    return true;
                };
                fields.into_iter().any(|(pattern, ty)| {
                    self.resolved_const_pattern_has_definite_mismatch(pattern, ty)
                })
            }
        }
    }

    pub(super) fn check_resolved_const_patterns(
        &mut self,
        patterns: &[ResolvedConstPattern],
        target_ty: InternedTyId,
    ) -> Option<()> {
        for pattern in patterns {
            match pattern.kind() {
                ResolvedConstPatternKind::Wildcard { .. }
                | ResolvedConstPatternKind::Bind { .. } => {}
                ResolvedConstPatternKind::Pointer { pattern, .. }
                | ResolvedConstPatternKind::MutPointer { pattern, .. } => {
                    let Some(TyKind::Pointer { elem, .. }) = self.ty_kind(target_ty) else {
                        return None;
                    };
                    self.check_resolved_const_patterns(std::slice::from_ref(pattern), elem)?;
                }
                ResolvedConstPatternKind::Expr(expr) => {
                    let target_ty = ConstValueType::Runtime(target_ty);
                    let pattern_ty = self
                        .resolved_const_expr_type(expr, Some(target_ty.runtime()?))
                        .or_else(|| self.resolved_const_expr_type(expr, None))?;
                    if pattern_ty != target_ty
                        && !self.const_equality_types_are_compatible(&target_ty, &pattern_ty)
                    {
                        return None;
                    }
                }
                ResolvedConstPatternKind::Range { start, end, .. } => {
                    if !self.is_integer_runtime_type(target_ty) {
                        return None;
                    }
                    let start_ty = self.resolved_const_arg_runtime_type(start, Some(target_ty))?;
                    let end_ty = self.resolved_const_arg_runtime_type(end, Some(target_ty))?;
                    if start_ty != target_ty || end_ty != target_ty {
                        return None;
                    }
                }
                ResolvedConstPatternKind::OptionalSome { pattern, .. } => {
                    let Some(TyKind::Optional { elem }) = self.ty_kind(target_ty) else {
                        return None;
                    };
                    self.check_resolved_const_patterns(std::slice::from_ref(pattern), elem)?;
                }
                ResolvedConstPatternKind::OptionalNull { .. } => {
                    if !matches!(self.ty_kind(target_ty), Some(TyKind::Optional { .. })) {
                        return None;
                    }
                }
                ResolvedConstPatternKind::ErrorOk { pattern, .. } => {
                    let Some(TyKind::ErrorUnion { value, .. }) = self.ty_kind(target_ty) else {
                        return None;
                    };
                    self.check_resolved_const_patterns(std::slice::from_ref(pattern), value)?;
                }
                ResolvedConstPatternKind::ErrorErr { pattern, .. } => {
                    let Some(TyKind::ErrorUnion { error, .. }) = self.ty_kind(target_ty) else {
                        return None;
                    };
                    self.check_resolved_const_patterns(std::slice::from_ref(pattern), error)?;
                }
                ResolvedConstPatternKind::EnumVariant {
                    variant, fields, ..
                } => {
                    for (pattern, ty) in
                        self.resolved_const_enum_pattern_fields(variant, fields, target_ty)?
                    {
                        self.check_resolved_const_patterns(std::slice::from_ref(pattern), ty)?;
                    }
                }
            }
        }
        Some(())
    }

    pub(super) fn bind_typed_resolved_const_patterns(
        &mut self,
        patterns: &[ResolvedConstPattern],
        target_ty: InternedTyId,
    ) -> Option<()> {
        for pattern in patterns {
            self.bind_typed_resolved_const_pattern(pattern, target_ty)?;
        }
        Some(())
    }

    pub(super) fn bind_typed_resolved_const_pattern(
        &mut self,
        pattern: &ResolvedConstPattern,
        target_ty: InternedTyId,
    ) -> Option<()> {
        match pattern.kind() {
            ResolvedConstPatternKind::Bind { local_id, .. } => {
                self.bind_const_local_type(*local_id, ConstValueType::Runtime(target_ty), false);
            }
            ResolvedConstPatternKind::Pointer { pattern, .. }
            | ResolvedConstPatternKind::MutPointer { pattern, .. } => {
                let Some(TyKind::Pointer { elem, .. }) = self.ty_kind(target_ty) else {
                    return None;
                };
                self.bind_typed_resolved_const_pattern(pattern, elem)?;
            }
            ResolvedConstPatternKind::OptionalSome { pattern, .. } => {
                let Some(TyKind::Optional { elem }) = self.ty_kind(target_ty) else {
                    return None;
                };
                self.bind_typed_resolved_const_pattern(pattern, elem)?;
            }
            ResolvedConstPatternKind::ErrorOk { pattern, .. } => {
                let Some(TyKind::ErrorUnion { value, .. }) = self.ty_kind(target_ty) else {
                    return None;
                };
                self.bind_typed_resolved_const_pattern(pattern, value)?;
            }
            ResolvedConstPatternKind::ErrorErr { pattern, .. } => {
                let Some(TyKind::ErrorUnion { error, .. }) = self.ty_kind(target_ty) else {
                    return None;
                };
                self.bind_typed_resolved_const_pattern(pattern, error)?;
            }
            ResolvedConstPatternKind::EnumVariant {
                variant, fields, ..
            } => {
                for (pattern, ty) in
                    self.resolved_const_enum_pattern_fields(variant, fields, target_ty)?
                {
                    self.bind_typed_resolved_const_pattern(pattern, ty)?;
                }
            }
            ResolvedConstPatternKind::Wildcard { .. }
            | ResolvedConstPatternKind::OptionalNull { .. }
            | ResolvedConstPatternKind::Expr(_)
            | ResolvedConstPatternKind::Range { .. } => {}
        }
        Some(())
    }

    pub(super) fn resolved_const_switch_arm_body_type(
        &mut self,
        body: &ResolvedConstSwitchArmBody,
        expected: Option<InternedTyId>,
    ) -> Option<ConstArmType> {
        match body.kind() {
            ResolvedConstSwitchArmBodyKind::Expr(expr) => self
                .resolved_const_expr_type(expr, expected)
                .map(ConstArmType::Value),
            ResolvedConstSwitchArmBodyKind::Block(block) => {
                self.resolved_const_switch_block_arm_type(block, expected)
            }
            ResolvedConstSwitchArmBodyKind::Stmt(stmt) => {
                self.resolved_const_stmt_arm_type(stmt, expected)
            }
        }
    }

    pub(super) fn resolved_const_switch_block_arm_type(
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

    pub(super) fn check_resolved_const_function_block(
        &mut self,
        block: &ResolvedConstBlock,
        return_type: Option<InternedTyId>,
    ) -> Option<()> {
        self.push_typed_const_scope();
        let result = (|| {
            for stmt in block.stmts() {
                self.check_resolved_const_stmt(stmt)?;
            }
            if let Some(tail) = block.tail() {
                self.check_const_function_result(tail, return_type, "const function body")?;
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

    fn const_function_types_match(&mut self, expected: InternedTyId, actual: InternedTyId) -> bool {
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
            _ => false,
        }
    }

    pub(super) fn check_resolved_const_stmt(
        &mut self,
        stmt: &nia_const_ir::ResolvedConstStmt,
    ) -> Option<()> {
        match stmt.kind() {
            ResolvedConstStmtKind::Binding(_)
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
                    .call_locals
                    .iter()
                    .rev()
                    .find_map(|frame| frame.return_type);
                self.check_const_function_result(expr, return_type, "const return value")
            }
            ResolvedConstStmtKind::Return(None) => {
                let return_type = self
                    .call_locals
                    .iter()
                    .rev()
                    .find_map(|frame| frame.return_type)
                    .map(|ty| self.substitute_ty_generics(ty));
                if return_type.is_some_and(|ty| {
                    !matches!(self.ty_kind(ty), Some(TyKind::Primitive(PrimitiveTy::Void)))
                }) {
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
                "const for-in expects an Iterator".to_string(),
            ));
            return None;
        };
        self.push_typed_const_scope();
        let result = (|| {
            let ConstValueType::Runtime(binding_ty) = binding_ty else {
                return None;
            };
            self.bind_typed_resolved_const_pattern(for_in.pattern(), binding_ty)?;
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
