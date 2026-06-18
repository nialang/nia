use super::ty_substitution::substitute_ty_generics_in_interner;
use super::*;

impl Analyzer<'_> {
    pub(super) fn check_resolved_comptime_bool_condition(
        &mut self,
        cond: &ResolvedComptimeExpr,
    ) -> Option<()> {
        let bool_ty = self.current_runtime_primitive_type(PrimitiveTy::Bool);
        let cond_ty = self.resolved_comptime_arg_runtime_type(cond, Some(bool_ty))?;
        (cond_ty == bool_ty).then_some(())
    }

    pub(super) fn push_typed_comptime_scope(&mut self) {
        self.call_locals.push(ComptimeCallFrame::default());
    }

    pub(super) fn pop_typed_comptime_scope(&mut self) {
        self.call_locals.pop();
    }

    pub(super) fn bind_comptime_local_type(&mut self, local_id: LocalId, ty: ComptimeValueType) {
        let Some(frame) = self.call_locals.last_mut() else {
            return;
        };
        frame.local_types.insert(local_id, ty);
    }

    pub(super) fn resolved_comptime_unary_expr_type(
        &mut self,
        op: ComptimeUnaryOp,
        inner: &ResolvedComptimeExpr,
    ) -> Option<ComptimeValueType> {
        match op {
            ComptimeUnaryOp::Not => {
                let bool_ty = self.current_runtime_primitive_type(PrimitiveTy::Bool);
                let inner_ty = self.resolved_comptime_arg_runtime_type(inner, Some(bool_ty))?;
                (inner_ty == bool_ty).then_some(ComptimeValueType::Runtime(bool_ty))
            }
            ComptimeUnaryOp::Neg => {
                let inner_ty = self.resolved_comptime_arg_runtime_type(inner, None)?;
                (self.is_integer_runtime_type(inner_ty) || self.is_float_runtime_type(inner_ty))
                    .then_some(ComptimeValueType::Runtime(inner_ty))
            }
            ComptimeUnaryOp::BitNot => {
                let inner_ty = self.resolved_comptime_arg_runtime_type(inner, None)?;
                self.is_integer_runtime_type(inner_ty)
                    .then_some(ComptimeValueType::Runtime(inner_ty))
            }
            ComptimeUnaryOp::Deref => {
                let inner_ty = self.resolved_comptime_arg_runtime_type(inner, None)?;
                let TyKind::Pointer { elem, .. } = self.ty_kind(inner_ty)? else {
                    return None;
                };
                self.import_ty_into_module_or_none(elem, self.current_execution_module_id())
                    .map(ComptimeValueType::Runtime)
            }
            ComptimeUnaryOp::RefReadOnly | ComptimeUnaryOp::Ref => None,
        }
    }

    pub(super) fn resolved_comptime_range_type(
        &mut self,
        range: &nia_comptime_ir::ResolvedComptimeRange,
        expected: Option<InternedTyId>,
    ) -> Option<ComptimeValueType> {
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
            Some(start) => Some(self.resolved_comptime_arg_runtime_type(start, expected_bound)?),
            None => None,
        };
        let end_ty = match range.end() {
            Some(end) => {
                Some(self.resolved_comptime_arg_runtime_type(end, expected_bound.or(start_ty))?)
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
            Some(bound) => Some(self.import_ty_into_module_or_none(bound, module_id)?),
            None => None,
        };
        let ty = self
            .working_interners
            .get_mut(&module_id)?
            .intern(TyKind::Range { kind, bound });
        Some(ComptimeValueType::Runtime(ty))
    }

    pub(super) fn resolved_comptime_binary_expr_type(
        &mut self,
        lhs: &ResolvedComptimeExpr,
        op: ComptimeBinaryOp,
        rhs: &ResolvedComptimeExpr,
    ) -> Option<ComptimeValueType> {
        match op {
            ComptimeBinaryOp::And | ComptimeBinaryOp::Or => {
                self.resolved_comptime_bool_binary_expr_type(lhs, rhs)
            }
            ComptimeBinaryOp::Eq | ComptimeBinaryOp::Ne => {
                self.resolved_comptime_equality_expr_type(lhs, rhs)
            }
            ComptimeBinaryOp::Lt
            | ComptimeBinaryOp::Le
            | ComptimeBinaryOp::Gt
            | ComptimeBinaryOp::Ge => {
                let lhs_ty = self.resolved_comptime_arg_runtime_type(lhs, None)?;
                let rhs_ty = self.resolved_comptime_arg_runtime_type(rhs, Some(lhs_ty))?;
                (lhs_ty == rhs_ty
                    && (self.is_integer_runtime_type(lhs_ty) || self.is_float_runtime_type(lhs_ty)))
                .then_some(ComptimeValueType::Runtime(
                    self.current_runtime_primitive_type(PrimitiveTy::Bool),
                ))
            }
            ComptimeBinaryOp::Mul
            | ComptimeBinaryOp::Div
            | ComptimeBinaryOp::Rem
            | ComptimeBinaryOp::Add
            | ComptimeBinaryOp::Sub
            | ComptimeBinaryOp::Shl
            | ComptimeBinaryOp::Shr
            | ComptimeBinaryOp::BitAnd
            | ComptimeBinaryOp::BitXor
            | ComptimeBinaryOp::BitOr => {
                let lhs_ty = self.resolved_comptime_arg_runtime_type(lhs, None)?;
                let rhs_ty = self.resolved_comptime_arg_runtime_type(rhs, Some(lhs_ty))?;
                let allowed = match op {
                    ComptimeBinaryOp::Shl
                    | ComptimeBinaryOp::Shr
                    | ComptimeBinaryOp::BitAnd
                    | ComptimeBinaryOp::BitXor
                    | ComptimeBinaryOp::BitOr => self.is_integer_runtime_type(lhs_ty),
                    _ => self.is_integer_runtime_type(lhs_ty) || self.is_float_runtime_type(lhs_ty),
                };
                (lhs_ty == rhs_ty && allowed).then_some(ComptimeValueType::Runtime(lhs_ty))
            }
        }
    }

    pub(super) fn resolved_comptime_cast_type(
        &mut self,
        expr: &ResolvedComptimeExpr,
        target: InternedTyId,
    ) -> Option<ComptimeValueType> {
        let source = self.resolved_comptime_arg_runtime_type(expr, None)?;
        let target =
            self.import_ty_into_module_or_none(target, self.current_execution_module_id())?;
        self.comptime_runtime_cast_is_supported(source, target)
            .then_some(ComptimeValueType::Runtime(target))
    }

    pub(super) fn comptime_runtime_cast_is_supported(
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

    pub(super) fn resolved_comptime_bool_binary_expr_type(
        &mut self,
        lhs: &ResolvedComptimeExpr,
        rhs: &ResolvedComptimeExpr,
    ) -> Option<ComptimeValueType> {
        let bool_ty = self.current_runtime_primitive_type(PrimitiveTy::Bool);
        let lhs_ty = self.resolved_comptime_arg_runtime_type(lhs, Some(bool_ty))?;
        let rhs_ty = self.resolved_comptime_arg_runtime_type(rhs, Some(bool_ty))?;
        (lhs_ty == bool_ty && rhs_ty == bool_ty).then_some(ComptimeValueType::Runtime(bool_ty))
    }

    pub(super) fn resolved_comptime_equality_expr_type(
        &mut self,
        lhs: &ResolvedComptimeExpr,
        rhs: &ResolvedComptimeExpr,
    ) -> Option<ComptimeValueType> {
        let lhs_ty = self.resolved_comptime_expr_type(lhs, None)?;
        let rhs_ty = self
            .resolved_comptime_expr_type(rhs, lhs_ty.runtime())
            .or_else(|| self.resolved_comptime_expr_type(rhs, None))?;
        (lhs_ty == rhs_ty || self.comptime_equality_types_are_compatible(&lhs_ty, &rhs_ty))
            .then_some(ComptimeValueType::Runtime(
                self.current_runtime_primitive_type(PrimitiveTy::Bool),
            ))
    }

    pub(super) fn comptime_equality_types_are_compatible(
        &mut self,
        lhs: &ComptimeValueType,
        rhs: &ComptimeValueType,
    ) -> bool {
        (matches!(lhs, ComptimeValueType::String) && self.is_runtime_char_array_type(rhs))
            || (matches!(rhs, ComptimeValueType::String) && self.is_runtime_char_array_type(lhs))
    }

    pub(super) fn is_runtime_char_array_type(&self, ty: &ComptimeValueType) -> bool {
        let ComptimeValueType::Runtime(ty) = ty else {
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
        self.source_interner_for_module(self.current_execution_module_id())
            .unwrap_or(self.input.interner)
            .primitive(primitive)
    }

    pub(super) fn comptime_string_literal_type(
        &mut self,
        literal: &ComptimeStringLiteral,
    ) -> Option<ComptimeValueType> {
        let len = nia_comptime_engine::eval_string_literal(literal)?
            .chars()
            .count() as u64;
        self.comptime_array_pointer_runtime_type(len, PrimitiveTy::Char)
            .map(ComptimeValueType::Runtime)
    }

    pub(super) fn comptime_byte_string_literal_type(
        &mut self,
        len: u64,
    ) -> Option<ComptimeValueType> {
        self.comptime_array_pointer_runtime_type(len, PrimitiveTy::U8)
            .map(ComptimeValueType::Runtime)
    }

    pub(super) fn comptime_array_pointer_runtime_type(
        &mut self,
        len: u64,
        elem_primitive: PrimitiveTy,
    ) -> Option<InternedTyId> {
        let array = self.comptime_array_runtime_type(len, elem_primitive)?;
        self.comptime_runtime_type(
            array,
            |elem| TyKind::Pointer {
                is_readonly: true,
                elem,
            },
            self.current_execution_module_id(),
        )
    }

    pub(super) fn comptime_array_runtime_type(
        &mut self,
        len: u64,
        elem_primitive: PrimitiveTy,
    ) -> Option<InternedTyId> {
        self.comptime_runtime_type(
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

    pub(super) fn resolved_comptime_call_return_type(
        &mut self,
        span: Span,
        callee: &ResolvedComptimeExpr,
        type_args: &[ResolvedComptimeTypeArg],
        args: &[ResolvedComptimeExpr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let function_id = self.resolved_comptime_function(callee)?;
        let signature = self
            .signatures_for_module(function_id.module_id)?
            .functions
            .get(&function_id.def_id)?
            .clone();
        let substitutions = self
            .instantiate_resolved_function_generics(
                span,
                function_id.module_id,
                &signature,
                type_args,
                args,
                expected,
            )
            .ok()?;
        self.resolved_call_type_substitutions
            .insert(span, substitutions.clone());
        self.substitute_ty_into_current_module(
            function_id.module_id,
            signature.return_type,
            &substitutions,
        )
    }

    pub(super) fn substitute_ty_into_current_module(
        &mut self,
        source_module_id: ModuleId,
        ty: InternedTyId,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Option<InternedTyId> {
        self.ensure_working_interner(source_module_id)?;
        let substituted = {
            let interner = self.working_interners.get_mut(&source_module_id)?;
            substitute_ty_generics_in_interner(interner, ty, &|generic| {
                substitutions.get(generic).copied()
            })
        };
        self.import_ty_into_module_or_none(substituted, self.current_execution_module_id())
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

    pub(super) fn comptime_runtime_type(
        &mut self,
        elem: InternedTyId,
        kind: impl FnOnce(InternedTyId) -> TyKind,
        target_module_id: ModuleId,
    ) -> Option<InternedTyId> {
        let imported_elem = self.import_ty_into_module_or_none(elem, target_module_id)?;
        self.working_interners
            .get_mut(&target_module_id)
            .map(|interner| interner.intern(kind(imported_elem)))
    }

    pub(super) fn comptime_error_union_type(
        &mut self,
        error: InternedTyId,
        value: InternedTyId,
    ) -> Option<InternedTyId> {
        let target_module_id = self.current_execution_module_id();
        let error = self.import_ty_into_module_or_none(error, target_module_id)?;
        let value = self.import_ty_into_module_or_none(value, target_module_id)?;
        self.working_interners
            .get_mut(&target_module_id)
            .map(|interner| interner.intern(TyKind::ErrorUnion { error, value }))
    }

    pub(super) fn call_local_type(&self, local_id: LocalId) -> Option<ComptimeValueType> {
        self.call_locals
            .iter()
            .rev()
            .find_map(|frame| frame.local_types.get(&local_id).cloned())
    }

    pub(super) fn builtin_comptime_type(&self) -> ComptimeValueType {
        builtin_comptime_value_type(self.current_runtime_primitive_type(PrimitiveTy::Usize))
    }
}
