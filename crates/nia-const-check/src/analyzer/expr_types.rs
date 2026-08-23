use super::ty_substitution::substitute_ty_generics;
use super::*;

impl Analyzer<'_> {
    pub(super) fn check_resolved_const_bool_condition(
        &mut self,
        cond: &ResolvedConstExpr,
    ) -> Option<()> {
        let bool_ty = self.current_runtime_primitive_type(PrimitiveTy::Bool);
        let Some(cond_ty) = self.resolved_const_arg_runtime_type(cond, Some(bool_ty)) else {
            return Some(());
        };
        if cond_ty != bool_ty && self.const_runtime_type_is_known(cond_ty) {
            self.push_const_type_error(cond.span(), "const condition must have type bool");
        }
        Some(())
    }

    pub(super) fn push_typed_const_scope(&mut self) {
        self.call_locals.push(ConstCallFrame::default());
    }

    pub(super) fn pop_typed_const_scope(&mut self) {
        self.call_locals.pop();
    }

    pub(super) fn bind_const_local_type(
        &mut self,
        local_id: LocalId,
        ty: ConstValueType,
        is_mutable: bool,
    ) {
        let Some(frame) = self.call_locals.last_mut() else {
            return;
        };
        frame.local_types.insert(local_id, ty);
        if is_mutable {
            frame.mutable_locals.insert(local_id);
        }
    }

    pub(super) fn const_local_is_mutable(&self, local_id: LocalId) -> Option<bool> {
        self.active_execution_frames().find_map(|frame| {
            frame
                .local_types
                .contains_key(&local_id)
                .then(|| frame.mutable_locals.contains(&local_id))
        })
    }

    pub(super) fn resolved_const_unary_expr_type(
        &mut self,
        op: ConstUnaryOp,
        inner: &ResolvedConstExpr,
        expected: Option<InternedTyId>,
    ) -> Option<ConstValueType> {
        match op {
            ConstUnaryOp::Not => {
                let bool_ty = self.current_runtime_primitive_type(PrimitiveTy::Bool);
                let inner_ty = self.resolved_const_arg_runtime_type(inner, Some(bool_ty))?;
                if inner_ty != bool_ty {
                    if self.const_runtime_type_is_known_builtin_scalar(inner_ty) {
                        self.push_const_type_error(
                            inner.span(),
                            "const logical not requires a bool operand",
                        );
                    }
                    return None;
                }
                Some(ConstValueType::Runtime(bool_ty))
            }
            ConstUnaryOp::Neg => {
                let inner_ty = self.resolved_const_arg_runtime_type(inner, expected)?;
                if !self.is_integer_runtime_type(inner_ty) && !self.is_float_runtime_type(inner_ty)
                {
                    if self.const_runtime_type_is_known_builtin_scalar(inner_ty) {
                        self.push_const_type_error(
                            inner.span(),
                            "const negation requires a numeric operand",
                        );
                    }
                    return None;
                }
                Some(ConstValueType::Runtime(inner_ty))
            }
            ConstUnaryOp::BitNot => {
                let inner_ty = self.resolved_const_arg_runtime_type(inner, expected)?;
                if !self.is_integer_runtime_type(inner_ty) {
                    if self.const_runtime_type_is_known_builtin_scalar(inner_ty) {
                        self.push_const_type_error(
                            inner.span(),
                            "const bitwise not requires an integer operand",
                        );
                    }
                    return None;
                }
                Some(ConstValueType::Runtime(inner_ty))
            }
            ConstUnaryOp::Deref => {
                let inner_ty = self.resolved_const_arg_runtime_type(inner, None)?;
                let TyKind::Pointer { elem, .. } = self.ty_kind(inner_ty)? else {
                    return None;
                };
                self.type_for_module_or_none(elem, self.current_execution_module_id())
                    .map(ConstValueType::Runtime)
            }
            ConstUnaryOp::RefReadOnly | ConstUnaryOp::Ref => {
                // A reference's expected pointee is the operand's context. This
                // must happen before structural inference so literals such as
                // `&[Item { ... }]` can inherit their nominal element type.
                let inner_expected = self.expected_const_ref_target(op, expected);
                let inner_ty = self.resolved_const_expr_type(inner, inner_expected)?;
                let is_readonly = matches!(op, ConstUnaryOp::RefReadOnly);
                let kind = match inner_ty {
                    ConstValueType::Array { elem, .. } => TyKind::Slice {
                        is_readonly,
                        elem: elem.runtime()?,
                    },
                    ConstValueType::Runtime(inner_ty) => match self.ty_kind(inner_ty)? {
                        TyKind::SlicePointee { elem } => TyKind::Slice { is_readonly, elem },
                        _ => TyKind::Pointer {
                            is_readonly,
                            elem: inner_ty,
                        },
                    },
                    ConstValueType::Int | ConstValueType::Bool | ConstValueType::String => {
                        return None;
                    }
                };
                let ty = self.intern_current_ty(kind)?;
                Some(ConstValueType::Runtime(ty))
            }
        }
    }

    fn expected_const_ref_target(
        &mut self,
        op: ConstUnaryOp,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        match (op, self.ty_kind(expected?)?) {
            (
                ConstUnaryOp::RefReadOnly,
                TyKind::Pointer {
                    is_readonly: true,
                    elem,
                },
            )
            | (
                ConstUnaryOp::Ref,
                TyKind::Pointer {
                    is_readonly: false,
                    elem,
                },
            ) => Some(elem),
            (ConstUnaryOp::RefReadOnly | ConstUnaryOp::Ref, TyKind::Slice { elem, .. }) => self
                .intern_current_ty(TyKind::Array {
                    len: ArrayLenTy::Infer,
                    elem,
                }),
            _ => None,
        }
    }

    pub(super) fn resolved_const_range_type(
        &mut self,
        range: &nia_const_ir::ResolvedConstRange,
        expected: Option<InternedTyId>,
    ) -> Option<ConstValueType> {
        let kind = match (
            range.start().is_some(),
            range.end().is_some(),
            range.is_inclusive(),
        ) {
            (true, true, false) => RangeTyKind::Exclusive,
            (true, true, true) => RangeTyKind::Inclusive,
            (true, false, false) => RangeTyKind::From,
            (false, true, false) => RangeTyKind::To,
            (false, true, true) => RangeTyKind::ToInclusive,
            (false, false, false) => RangeTyKind::Full,
            (true, false, true) | (false, false, true) => return None,
        };
        let expected_bound = expected.and_then(|expected| match self.ty_kind(expected) {
            Some(TyKind::Range {
                kind: expected_kind,
                bound,
            }) if expected_kind == kind => bound,
            _ => None,
        });
        let start_ty = match range.start() {
            Some(start) => Some(self.resolved_const_arg_runtime_type(start, expected_bound)?),
            None => None,
        };
        let end_ty = match range.end() {
            Some(end) => {
                Some(self.resolved_const_arg_runtime_type(end, expected_bound.or(start_ty))?)
            }
            None => None,
        };
        let bound = match (start_ty, end_ty) {
            (Some(start_ty), Some(end_ty)) if start_ty == end_ty => Some(start_ty),
            (Some(bound), None) | (None, Some(bound)) => Some(bound),
            (None, None) => expected_bound,
            (Some(_), Some(_)) => return None,
        };
        let module_id = self.current_execution_module_id();
        let bound = match bound {
            Some(bound) => Some(self.type_for_module_or_none(bound, module_id)?),
            None => None,
        };
        let ty = self
            .type_contexts
            .get_mut(&module_id)?
            .intern(TyKind::Range { kind, bound });
        Some(ConstValueType::Runtime(ty))
    }

    pub(super) fn resolved_const_binary_expr_type(
        &mut self,
        lhs: &ResolvedConstExpr,
        op: ConstBinaryOp,
        rhs: &ResolvedConstExpr,
        expected: Option<InternedTyId>,
    ) -> Option<ConstValueType> {
        match op {
            ConstBinaryOp::And | ConstBinaryOp::Or => {
                self.resolved_const_bool_binary_expr_type(lhs, rhs)
            }
            ConstBinaryOp::Eq | ConstBinaryOp::Ne => {
                self.resolved_const_equality_expr_type(lhs, rhs)
            }
            ConstBinaryOp::Lt | ConstBinaryOp::Le | ConstBinaryOp::Gt | ConstBinaryOp::Ge => {
                let lhs_ty = self.resolved_const_arg_runtime_type(lhs, None)?;
                let rhs_ty = self.resolved_const_arg_runtime_type(rhs, Some(lhs_ty))?;
                if lhs_ty != rhs_ty
                    || (!self.is_integer_runtime_type(lhs_ty)
                        && !self.is_float_runtime_type(lhs_ty))
                {
                    self.push_known_const_binary_type_error(lhs_ty, rhs, rhs_ty);
                    return None;
                }
                Some(ConstValueType::Runtime(
                    self.current_runtime_primitive_type(PrimitiveTy::Bool),
                ))
            }
            ConstBinaryOp::Mul
            | ConstBinaryOp::Div
            | ConstBinaryOp::Rem
            | ConstBinaryOp::Add
            | ConstBinaryOp::Sub
            | ConstBinaryOp::Shl
            | ConstBinaryOp::Shr
            | ConstBinaryOp::BitAnd
            | ConstBinaryOp::BitXor
            | ConstBinaryOp::BitOr => {
                let lhs_ty = self.resolved_const_arg_runtime_type(lhs, expected)?;
                let rhs_ty = self.resolved_const_arg_runtime_type(rhs, Some(lhs_ty))?;
                let allowed = match op {
                    ConstBinaryOp::Shl
                    | ConstBinaryOp::Shr
                    | ConstBinaryOp::BitAnd
                    | ConstBinaryOp::BitXor
                    | ConstBinaryOp::BitOr => self.is_integer_runtime_type(lhs_ty),
                    _ => self.is_integer_runtime_type(lhs_ty) || self.is_float_runtime_type(lhs_ty),
                };
                if lhs_ty != rhs_ty || !allowed {
                    self.push_known_const_binary_type_error(lhs_ty, rhs, rhs_ty);
                    return None;
                }
                Some(ConstValueType::Runtime(lhs_ty))
            }
        }
    }

    pub(super) fn resolved_const_cast_type(
        &mut self,
        expr: &ResolvedConstExpr,
        target: InternedTyId,
    ) -> Option<ConstValueType> {
        let source = self.resolved_const_arg_runtime_type(expr, None)?;
        let target = self.type_for_module_or_none(target, self.current_execution_module_id())?;
        self.const_runtime_cast_is_supported(source, target)
            .then_some(ConstValueType::Runtime(target))
    }

    pub(super) fn const_runtime_cast_is_supported(
        &mut self,
        source: InternedTyId,
        target: InternedTyId,
    ) -> bool {
        let Some(TyKind::Primitive(source)) = self.ty_kind(source) else {
            return false;
        };
        let Some(TyKind::Primitive(target)) = self.ty_kind(target) else {
            return false;
        };
        let source_numeric = primitive_integer_layout(source, self.input.target.pointer_width)
            .is_some()
            || is_float_primitive(source);
        let target_numeric = primitive_integer_layout(target, self.input.target.pointer_width)
            .is_some()
            || is_float_primitive(target);
        source_numeric && target_numeric
    }

    pub(super) fn resolved_const_bool_binary_expr_type(
        &mut self,
        lhs: &ResolvedConstExpr,
        rhs: &ResolvedConstExpr,
    ) -> Option<ConstValueType> {
        let bool_ty = self.current_runtime_primitive_type(PrimitiveTy::Bool);
        let lhs_ty = self.resolved_const_arg_runtime_type(lhs, Some(bool_ty))?;
        let rhs_ty = self.resolved_const_arg_runtime_type(rhs, Some(bool_ty))?;
        if lhs_ty != bool_ty || rhs_ty != bool_ty {
            self.push_known_const_binary_type_error(lhs_ty, rhs, rhs_ty);
            return None;
        }
        Some(ConstValueType::Runtime(bool_ty))
    }

    pub(super) fn resolved_const_equality_expr_type(
        &mut self,
        lhs: &ResolvedConstExpr,
        rhs: &ResolvedConstExpr,
    ) -> Option<ConstValueType> {
        let lhs_ty = self.resolved_const_expr_type(lhs, None)?;
        let rhs_ty = self
            .resolved_const_expr_type(rhs, lhs_ty.runtime())
            .or_else(|| self.resolved_const_expr_type(rhs, None))?;
        if lhs_ty != rhs_ty && !self.const_equality_types_are_compatible(&lhs_ty, &rhs_ty) {
            if let (Some(lhs_ty), Some(rhs_ty)) = (lhs_ty.runtime(), rhs_ty.runtime()) {
                self.push_known_const_binary_type_error(lhs_ty, rhs, rhs_ty);
            }
            return None;
        }
        Some(ConstValueType::Runtime(
            self.current_runtime_primitive_type(PrimitiveTy::Bool),
        ))
    }

    fn push_known_const_binary_type_error(
        &mut self,
        lhs_ty: InternedTyId,
        rhs: &ResolvedConstExpr,
        rhs_ty: InternedTyId,
    ) {
        if self.const_runtime_type_is_known_builtin_scalar(lhs_ty)
            && self.const_runtime_type_is_known_builtin_scalar(rhs_ty)
        {
            self.push_const_type_error(rhs.span(), "const operator has incompatible operand types");
        }
    }

    pub(super) fn const_runtime_type_is_known(&self, ty: InternedTyId) -> bool {
        !self.type_contains_generic(ty)
            && !matches!(
                self.ty_kind(ty),
                Some(TyKind::Error | TyKind::Projection { .. }) | None
            )
    }

    fn const_runtime_type_is_known_builtin_scalar(&self, ty: InternedTyId) -> bool {
        self.const_runtime_type_is_known(ty)
            && matches!(self.ty_kind(ty), Some(TyKind::Primitive(_)))
    }

    pub(super) fn push_const_type_error(&mut self, span: Span, message: &str) {
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            span,
            message.to_string(),
        ));
    }

    pub(super) fn const_equality_types_are_compatible(
        &mut self,
        lhs: &ConstValueType,
        rhs: &ConstValueType,
    ) -> bool {
        (matches!(lhs, ConstValueType::String) && self.is_runtime_char_array_type(rhs))
            || (matches!(rhs, ConstValueType::String) && self.is_runtime_char_array_type(lhs))
    }

    pub(super) fn is_runtime_char_array_type(&self, ty: &ConstValueType) -> bool {
        let ConstValueType::Runtime(ty) = ty else {
            return false;
        };
        let Some(TyKind::Array { elem, .. }) = self.ty_kind(*ty) else {
            return false;
        };
        matches!(
            self.ty_kind(elem),
            Some(TyKind::Primitive(PrimitiveTy::Char))
        )
    }

    pub(super) fn current_runtime_primitive_type(&self, primitive: PrimitiveTy) -> InternedTyId {
        self.primitive_ty_for_module(self.current_execution_module_id(), primitive)
    }

    pub(super) fn current_runtime_tuple_type(&self, elems: Vec<InternedTyId>) -> InternedTyId {
        self.type_contexts
            .get(&self.current_execution_module_id())
            .expect("active const execution module has a type context")
            .intern(TyKind::Tuple(elems))
    }

    pub(super) fn const_string_literal_type(
        &mut self,
        literal: &ConstStringLiteral,
    ) -> Option<ConstValueType> {
        let len = u64::try_from(
            nia_const_eval::eval_string_literal(literal)?
                .chars()
                .count(),
        )
        .ok()?;
        self.const_array_runtime_type(len, PrimitiveTy::Char)
            .map(ConstValueType::Runtime)
    }

    pub(super) fn const_byte_string_literal_type(&mut self, len: u64) -> Option<ConstValueType> {
        self.const_array_runtime_type(len, PrimitiveTy::U8)
            .map(ConstValueType::Runtime)
    }

    pub(super) fn const_array_runtime_type(
        &mut self,
        len: u64,
        elem_primitive: PrimitiveTy,
    ) -> Option<InternedTyId> {
        self.const_runtime_type(
            self.current_runtime_primitive_type(elem_primitive),
            |elem| TyKind::Array {
                len: ArrayLenTy::ConstValue(len),
                elem,
            },
            self.current_execution_module_id(),
        )
    }

    pub(super) fn is_integer_runtime_type(&self, ty: InternedTyId) -> bool {
        matches!(
            self.ty_kind(ty),
            Some(TyKind::Primitive(
                PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::I128
                    | PrimitiveTy::Isize
                    | PrimitiveTy::U8
                    | PrimitiveTy::U16
                    | PrimitiveTy::U32
                    | PrimitiveTy::U64
                    | PrimitiveTy::U128
                    | PrimitiveTy::Usize
            ))
        )
    }

    pub(super) fn is_float_runtime_type(&self, ty: InternedTyId) -> bool {
        matches!(
            self.ty_kind(ty),
            Some(TyKind::Primitive(PrimitiveTy::F32 | PrimitiveTy::F64))
        )
    }

    pub(super) fn resolved_const_call_return_type(
        &mut self,
        span: Span,
        callee: &ResolvedConstExpr,
        generic_args: &[ResolvedConstGenericArg],
        args: &[ResolvedConstExpr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        if let ResolvedConstExprKind::Method { receiver, .. } = callee.kind()
            && self
                .resolved_const_arg_runtime_type(receiver, None)
                .is_none()
        {
            return None;
        }
        let resolved_callee = match self.resolved_const_callee(callee) {
            ResolvedConstCalleeSelection::Unique(callee) => callee,
            ResolvedConstCalleeSelection::NoMatch => {
                if let Some((_, bound)) =
                    self.resolved_const_range_method(callee, generic_args, args)
                {
                    return bound.or(expected);
                }
                if let Some((mutable, elem)) =
                    self.resolved_const_slice_pointer_method(callee, generic_args, args)
                {
                    let output = self.intern_current_ty(TyKind::Pointer {
                        is_readonly: !mutable,
                        elem,
                    })?;
                    return Some(output);
                }
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::CONST,
                    span,
                    "const expression can only call `const fn`".to_string(),
                ));
                return None;
            }
            ResolvedConstCalleeSelection::Ambiguous => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::CONST,
                    span,
                    "ambiguous const method call",
                ));
                return None;
            }
        };
        let function_id = resolved_callee.function_id;
        let signature = self
            .function_signatures_for_module(function_id.module_id)?
            .as_ref()
            .functions
            .get(&function_id.def_id)?
            .clone();
        if !signature.is_const {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::CONST,
                span,
                "const expression can only call `const fn`".to_string(),
            ));
            return None;
        }
        let call_args = resolved_callee
            .receiver
            .into_iter()
            .chain(args.iter().cloned())
            .collect::<Vec<_>>();
        let instantiation = self
            .instantiate_resolved_function_generics(
                span,
                ConstFunctionInstantiationInput {
                    signature_module_id: function_id.module_id,
                    signature: &signature,
                    generic_args,
                    arg_exprs: &call_args,
                    expected_return: expected,
                    initial: resolved_callee.target_instantiation,
                },
            )
            .ok()?;
        self.resolved_call_instantiations
            .insert(span, instantiation.clone());
        if let Some(return_ty) =
            self.builtin_function_call_return_type(&signature, &call_args, expected)
        {
            return Some(return_ty);
        }
        self.substitute_ty_into_current_module(
            function_id.module_id,
            signature.return_type,
            &instantiation.type_substitutions,
        )
    }

    fn builtin_function_call_return_type(
        &mut self,
        signature: &FunctionSignature,
        args: &[ResolvedConstExpr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let builtin = signature
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                FunctionAttribute::Builtin(builtin) => Some(*builtin),
                FunctionAttribute::Naked => None,
            })?;
        match builtin {
            BuiltinFunction::ConstError => expected,
            BuiltinFunction::Embed => {
                let [path] = args else {
                    return None;
                };
                let ResolvedConstExprKind::String(path) = path.kind() else {
                    return None;
                };
                let path = nia_const_eval::eval_string_literal(path)?;
                let resolved = super::env_impl::resolve_embed_path(
                    self.current_execution_source_path()?.as_str(),
                    &path,
                );
                let len = std::fs::metadata(resolved).ok()?.len();
                self.const_byte_string_literal_type(len).and_then(|ty| {
                    let ConstValueType::Runtime(ty) = ty else {
                        return None;
                    };
                    Some(ty)
                })
            }
            _ => None,
        }
    }

    pub(super) fn substitute_ty_into_current_module(
        &mut self,
        source_module_id: ModuleId,
        ty: InternedTyId,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> Option<InternedTyId> {
        self.ensure_type_context(source_module_id)?;
        let substituted = {
            let types = self.type_contexts.get(&source_module_id)?;
            substitute_ty_generics(types, ty, &|generic| substitutions.get(generic).copied())
        };
        self.type_for_module_or_none(substituted, self.current_execution_module_id())
    }

    pub(super) fn expected_error_union_parts(
        &self,
        expected: InternedTyId,
    ) -> Option<(InternedTyId, InternedTyId)> {
        match self.ty_kind(expected) {
            Some(TyKind::ErrorUnion { error, value }) => Some((error, value)),
            _ => None,
        }
    }

    pub(super) fn const_runtime_type(
        &mut self,
        elem: InternedTyId,
        kind: impl FnOnce(InternedTyId) -> TyKind,
        target_module_id: ModuleId,
    ) -> Option<InternedTyId> {
        let elem = self.type_for_module_or_none(elem, target_module_id)?;
        self.type_contexts
            .get(&target_module_id)
            .map(|types| types.intern(kind(elem)))
    }

    pub(super) fn const_error_union_type(
        &mut self,
        error: InternedTyId,
        value: InternedTyId,
    ) -> Option<InternedTyId> {
        let target_module_id = self.current_execution_module_id();
        let error = self.type_for_module_or_none(error, target_module_id)?;
        let value = self.type_for_module_or_none(value, target_module_id)?;
        self.type_contexts
            .get(&target_module_id)
            .map(|types| types.intern(TyKind::ErrorUnion { error, value }))
    }

    pub(super) fn call_local_type(&self, local_id: LocalId) -> Option<ConstValueType> {
        self.active_execution_frames()
            .find_map(|frame| frame.local_types.get(&local_id).cloned())
    }
}
