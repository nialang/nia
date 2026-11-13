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

    /// Resolve the type of a named local initializer.
    ///
    /// Standalone const expressions intentionally preserve unsuffixed integer
    /// literals as `ConstValueType::Int` until an expected type is available.
    /// A local binding is a type-inference boundary, however: when no explicit
    /// type or other expected type applies, Nia's language rule defaults that
    /// integer to `i32`. Keeping this default here lets later expressions,
    /// including references to the local, observe the same runtime type as the
    /// ordinary body checker.
    pub(super) fn resolved_const_binding_type(
        &mut self,
        binding: &nia_const_ir::ResolvedConstBinding,
    ) -> Option<ConstValueType> {
        let explicit = binding
            .explicit_type()
            .map(|ty| self.substitute_ty_generics(ty));
        let inferred = self.resolved_const_expr_type(binding.value(), explicit);
        if inferred.is_some() {
            return inferred;
        }
        if explicit.is_some() {
            return None;
        }
        self.resolved_const_expr_with_numeric_defaults(binding.value())
    }

    pub(super) fn is_untyped_const_integer_expr(&self, expr: &ResolvedConstExpr) -> bool {
        match expr.kind() {
            ResolvedConstExprKind::Integer(text) => integer_literal_suffix_ty(text).is_none(),
            ResolvedConstExprKind::Unary {
                op: ConstUnaryOp::Neg,
                expr,
            } => matches!(
                expr.kind(),
                ResolvedConstExprKind::Integer(text)
                    if integer_literal_suffix_ty(text).is_none()
            ),
            _ => false,
        }
    }

    pub(super) fn is_untyped_const_numeric_expr(&self, expr: &ResolvedConstExpr) -> bool {
        self.untyped_const_numeric_default_primitive(expr).is_some()
    }

    pub(super) fn untyped_const_numeric_default_primitive(
        &self,
        expr: &ResolvedConstExpr,
    ) -> Option<PrimitiveTy> {
        match expr.kind() {
            ResolvedConstExprKind::Integer(text) if integer_literal_suffix_ty(text).is_none() => {
                Some(PrimitiveTy::I32)
            }
            ResolvedConstExprKind::Float(text) if float_literal_suffix_ty(text).is_none() => {
                Some(PrimitiveTy::F64)
            }
            ResolvedConstExprKind::Unary {
                op: ConstUnaryOp::Neg,
                expr,
            } => match expr.kind() {
                ResolvedConstExprKind::Integer(text)
                    if integer_literal_suffix_ty(text).is_none() =>
                {
                    Some(PrimitiveTy::I32)
                }
                ResolvedConstExprKind::Float(text) if float_literal_suffix_ty(text).is_none() => {
                    Some(PrimitiveTy::F64)
                }
                _ => None,
            },
            _ => None,
        }
    }

    pub(super) fn const_local_is_mutable(&self, local_id: LocalId) -> Option<bool> {
        self.active_execution_frames().find_map(|frame| {
            // Type-only frames have `local_types`; evaluation frames may keep
            // an inferred literal only in their value/allocation maps. These
            // are separate facts from mutability, which is recorded uniformly
            // in `mutable_locals`.
            let owns_local = frame.local_types.contains_key(&local_id)
                || frame.locals.contains_key(&local_id)
                || frame.allocation_ids.contains_key(&local_id);
            owns_local.then(|| frame.mutable_locals.contains(&local_id))
        })
    }

    pub(super) fn resolved_const_place_is_writable(&mut self, expr: &ResolvedConstExpr) -> bool {
        match expr.kind() {
            ResolvedConstExprKind::Name(ConstNameResolution::Local(local_id)) => {
                self.const_local_is_mutable(*local_id) == Some(true)
            }
            ResolvedConstExprKind::Field { lhs, .. }
            | ResolvedConstExprKind::Index { lhs, .. }
            | ResolvedConstExprKind::Slice { lhs, .. } => {
                if let Some(ty) = self.resolved_const_arg_runtime_type(lhs, None)
                    && let Some(
                        TyKind::Pointer { is_readonly, .. } | TyKind::Slice { is_readonly, .. },
                    ) = self.ty_kind(ty)
                {
                    return !is_readonly;
                }
                self.resolved_const_place_is_writable(lhs)
            }
            ResolvedConstExprKind::Unary {
                op: ConstUnaryOp::Deref,
                expr: inner,
            } => self
                .resolved_const_arg_runtime_type(inner, None)
                .and_then(|ty| self.ty_kind(ty))
                .is_some_and(|kind| {
                    matches!(
                        kind,
                        TyKind::Pointer {
                            is_readonly: false,
                            ..
                        }
                    )
                }),
            _ => false,
        }
    }

    pub(super) fn resolved_const_unary_expr_type(
        &mut self,
        span: Span,
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
                let inner_ty = match inner.kind() {
                    ResolvedConstExprKind::Integer(text) => self
                        .resolved_const_integer_literal_type(span, text, expected, true)?
                        .runtime()?,
                    _ => self.resolved_const_arg_runtime_type(inner, expected)?,
                };
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
        let (start_ty, end_ty) = match (range.start(), range.end(), expected_bound) {
            (Some(start), Some(end), None)
                if self.is_untyped_const_integer_expr(start)
                    && !self.is_untyped_const_integer_expr(end) =>
            {
                let end_ty = self.resolved_const_range_bound_runtime_type(end, None);
                let start_ty = self.resolved_const_range_bound_runtime_type(start, end_ty);
                (start_ty, end_ty)
            }
            (Some(start), Some(end), None)
                if !self.is_untyped_const_integer_expr(start)
                    && self.is_untyped_const_integer_expr(end) =>
            {
                let start_ty = self.resolved_const_range_bound_runtime_type(start, None);
                let end_ty = self.resolved_const_range_bound_runtime_type(end, start_ty);
                (start_ty, end_ty)
            }
            _ => {
                let start_ty = range.start().and_then(|start| {
                    self.resolved_const_range_bound_runtime_type(start, expected_bound)
                });
                let end_ty = range.end().and_then(|end| {
                    self.resolved_const_range_bound_runtime_type(end, expected_bound.or(start_ty))
                });
                (start_ty, end_ty)
            }
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
            .type_context(module_id)
            .intern(TyKind::Range { kind, bound });
        Some(ConstValueType::Runtime(ty))
    }

    fn resolved_const_range_bound_runtime_type(
        &mut self,
        expr: &ResolvedConstExpr,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        self.resolved_const_arg_runtime_type(expr, expected)
            .or_else(|| {
                if expected.is_some() || !self.is_untyped_const_integer_expr(expr) {
                    return None;
                }
                let default_int = self.current_runtime_primitive_type(PrimitiveTy::I32);
                self.resolved_const_arg_runtime_type(expr, Some(default_int))
            })
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
                let (lhs_ty, rhs_ty) =
                    self.resolved_const_binary_operand_types(lhs, rhs, expected)?;
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
                let (lhs_ty, rhs_ty) =
                    self.resolved_const_binary_operand_types(lhs, rhs, expected)?;
                let allowed = match op {
                    ConstBinaryOp::Shl
                    | ConstBinaryOp::Shr
                    | ConstBinaryOp::BitAnd
                    | ConstBinaryOp::BitXor
                    | ConstBinaryOp::BitOr => self.is_integer_runtime_type(lhs_ty),
                    _ => self.is_integer_runtime_type(lhs_ty) || self.is_float_runtime_type(lhs_ty),
                };
                let same_type_required = !matches!(op, ConstBinaryOp::Shl | ConstBinaryOp::Shr);
                if (same_type_required && lhs_ty != rhs_ty) || !allowed {
                    self.push_known_const_binary_type_error(lhs_ty, rhs, rhs_ty);
                    return None;
                }
                Some(ConstValueType::Runtime(lhs_ty))
            }
        }
    }

    fn resolved_const_binary_operand_types(
        &mut self,
        lhs: &ResolvedConstExpr,
        rhs: &ResolvedConstExpr,
        expected: Option<InternedTyId>,
    ) -> Option<(InternedTyId, InternedTyId)> {
        if expected.is_some() {
            let lhs_ty = self.resolved_const_arg_runtime_type(lhs, expected)?;
            let rhs_ty = self.resolved_const_arg_runtime_type(rhs, Some(lhs_ty))?;
            return Some((lhs_ty, rhs_ty));
        }
        if self.is_untyped_const_numeric_expr(lhs) && !self.is_untyped_const_numeric_expr(rhs) {
            let rhs_ty = self.resolved_const_arg_runtime_type(rhs, None)?;
            let lhs_ty = self.resolved_const_arg_runtime_type(lhs, Some(rhs_ty))?;
            return Some((lhs_ty, rhs_ty));
        }
        if !self.is_untyped_const_numeric_expr(lhs) && self.is_untyped_const_numeric_expr(rhs) {
            let lhs_ty = self.resolved_const_arg_runtime_type(lhs, None)?;
            let rhs_ty = self.resolved_const_arg_runtime_type(rhs, Some(lhs_ty))?;
            return Some((lhs_ty, rhs_ty));
        }
        let lhs_ty = self.resolved_const_arg_runtime_type(lhs, None)?;
        let rhs_ty = self.resolved_const_arg_runtime_type(rhs, Some(lhs_ty))?;
        Some((lhs_ty, rhs_ty))
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
        let source_numeric = primitive_integer_layout(source, self.input.target.pointer_width())
            .is_some()
            || is_float_primitive(source);
        let target_numeric = primitive_integer_layout(target, self.input.target.pointer_width())
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
        let (lhs_ty, rhs_ty) = if self.is_untyped_const_numeric_expr(lhs)
            && !self.is_untyped_const_numeric_expr(rhs)
        {
            let rhs_ty = self.resolved_const_arg_runtime_type(rhs, None)?;
            let lhs_ty = self.resolved_const_expr_type(lhs, Some(rhs_ty))?;
            (lhs_ty, ConstValueType::Runtime(rhs_ty))
        } else if !self.is_untyped_const_numeric_expr(lhs)
            && self.is_untyped_const_numeric_expr(rhs)
        {
            let lhs_ty = self.resolved_const_expr_type(lhs, None)?;
            let rhs_ty = self.resolved_const_expr_type(rhs, lhs_ty.runtime())?;
            (lhs_ty, rhs_ty)
        } else {
            let lhs_ty = self.resolved_const_expr_type(lhs, None).or_else(|| {
                self.resolved_const_expr_type(
                    lhs,
                    Some(self.current_runtime_primitive_type(PrimitiveTy::I32)),
                )
            })?;
            let rhs_ty = self
                .resolved_const_expr_type(rhs, lhs_ty.runtime())
                .or_else(|| self.resolved_const_expr_type(rhs, None))?;
            (lhs_ty, rhs_ty)
        };
        if !self.const_equality_types_are_compatible(&lhs_ty, &rhs_ty) {
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
        self.diagnostics.push(
            Diagnostic::user_error(codes::TYPE_CHECK, message)
                .primary(span, message)
                .help("adjust the constant expression to match the expected type")
                .finish(),
        );
    }

    /// Whether values of the two types can be compared or matched. Runtime
    /// types compare by normalized identity: an alias is its target, so a
    /// value built as `Alias::A` and one declared `: Alias` have one type.
    pub(super) fn const_equality_types_are_compatible(
        &mut self,
        lhs: &ConstValueType,
        rhs: &ConstValueType,
    ) -> bool {
        if lhs == rhs {
            return true;
        }
        if let (ConstValueType::Runtime(lhs), ConstValueType::Runtime(rhs)) = (lhs, rhs)
            && self.normalized_ty(*lhs) == self.normalized_ty(*rhs)
        {
            return true;
        }
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
        self.intern_type_for_module(self.current_execution_module_id(), TyKind::Tuple(elems))
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
                    "const expression can only call `const fun`".to_string(),
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
        let signature = self.function_signature(function_id)?;
        if !signature.is_const {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::CONST,
                span,
                "const expression can only call `const fun`".to_string(),
            ));
            return None;
        }
        let call_args = resolved_callee
            .receiver
            .into_iter()
            .map(|receiver| *receiver)
            .chain(args.iter().cloned())
            .collect::<Vec<_>>();
        let specialized_return = super::env_impl::builtin_function(&signature)
            .is_some_and(BuiltinFunction::specializes_return_type);
        let instantiation = match self.instantiate_resolved_function_generics(
            span,
            ConstFunctionInstantiationInput {
                signature_module_id: function_id.module_id,
                signature: &signature,
                generic_args,
                arg_exprs: &call_args,
                expected_return: expected.filter(|_| !specialized_return),
                initial: resolved_callee.target_instantiation,
            },
        ) {
            Ok(instantiation) => instantiation,
            Err(error) => {
                let cause = matches!(error, ConstInstantiationError::ReturnTypeMismatch(_))
                    .then(|| self.body_type_check_cause(span));
                let error: ConstError = error.into();
                let mut diagnostic =
                    Diagnostic::user_error_at(codes::CONST, error.span, error.message);
                diagnostic.cause = cause;
                self.diagnostics.push(diagnostic);
                return None;
            }
        };
        self.resolved_call_instantiations
            .insert(span, instantiation.clone());
        if let Some(return_ty) =
            self.builtin_function_call_return_type(&signature, &instantiation, &call_args, expected)
        {
            return Some(return_ty);
        }
        self.substitute_ty_into_current_module(
            function_id.module_id,
            signature.return_type,
            &instantiation.type_substitutions,
            &instantiation.const_substitutions,
        )
    }

    fn builtin_function_call_return_type(
        &mut self,
        signature: &FunctionSignature,
        instantiation: &ConstGenericInstantiation,
        args: &[ResolvedConstExpr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        match super::env_impl::builtin_function(signature)? {
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
            // `shuffle[V, N: usize](first: V, second: V, indices: [usize; N])`
            // gives a vector of `V`'s lane type with `N` lanes.
            BuiltinFunction::Shuffle => {
                let [vector_param, lanes_param] = signature.generic_params.as_slice() else {
                    return None;
                };
                let vector_ty = *instantiation.type_substitutions.get(&vector_param.name)?;
                let TyKind::Vector { elem, .. } = self.ty_kind(vector_ty)? else {
                    return None;
                };
                let lanes = match &instantiation
                    .const_substitutions
                    .get(&lanes_param.name)?
                    .value
                {
                    ConstGenericValue::Int(lanes) => u32::try_from(lanes.bits()).ok()?,
                    _ => return None,
                };
                self.intern_current_ty(TyKind::Vector { elem, lanes })
            }
            _ => None,
        }
    }

    pub(super) fn substitute_ty_into_current_module(
        &mut self,
        source_module_id: ModuleId,
        ty: InternedTyId,
        type_substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> Option<InternedTyId> {
        let substituted = {
            let types = self.type_context(source_module_id);
            types.substitute(
                ty,
                &|generic| type_substitutions.get(generic).copied(),
                &|generic| const_substitutions.get(generic).cloned(),
                None,
            )
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
        Some(self.type_context(target_module_id).intern(kind(elem)))
    }

    pub(super) fn const_error_union_type(
        &mut self,
        error: InternedTyId,
        value: InternedTyId,
    ) -> Option<InternedTyId> {
        let target_module_id = self.current_execution_module_id();
        let error = self.type_for_module_or_none(error, target_module_id)?;
        let value = self.type_for_module_or_none(value, target_module_id)?;
        Some(
            self.type_context(target_module_id)
                .intern(TyKind::ErrorUnion { error, value }),
        )
    }

    pub(super) fn call_local_type(&self, local_id: LocalId) -> Option<ConstValueType> {
        self.active_execution_frames()
            .find_map(|frame| frame.local_types.get(&local_id).cloned())
    }
}
