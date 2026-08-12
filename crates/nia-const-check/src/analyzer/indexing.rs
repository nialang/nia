use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConstSliceLenCheck {
    Known(u64),
    // A runtime-dependent bound or unresolved array length must not become an error here.
    Unknown,
    // The range is structurally invalid, so downstream inference must not use it.
    Invalid,
}

impl Analyzer<'_> {
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

    fn resolved_const_runtime_index_type(
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

    pub(super) fn visit_resolved_const_index_operand(&mut self, index: &ResolvedConstExpr) {
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

    fn resolved_const_runtime_slice_type(
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

    fn const_slice_result_type(
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

    pub(super) fn visit_resolved_const_slice_bounds(
        &mut self,
        range: &nia_const_ir::ResolvedConstSliceRange,
    ) {
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

        // An open end normally inherits the source length. When that length is
        // unresolved, contextual array length still determines `start..`.
        let end = match explicit_end {
            Some(ConstSliceLenCheck::Known(end)) => Some(end),
            Some(ConstSliceLenCheck::Unknown) => None,
            Some(ConstSliceLenCheck::Invalid) => unreachable!(),
            None => source_len.or_else(|| expected_len.and_then(|len| start.checked_add(len))),
        };
        let Some(mut end) = end else {
            return ConstSliceLenCheck::Unknown;
        };

        // Normalize inclusive ranges to a half-open interval before validating
        // order, source bounds, and the resulting length.
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

    fn expected_const_array_len(&mut self, expected: Option<InternedTyId>) -> Option<u64> {
        let expected = expected?;
        let TyKind::Array { len, .. } = self.ty_kind(expected)? else {
            return None;
        };
        self.array_len_const_value(len)
    }
}
