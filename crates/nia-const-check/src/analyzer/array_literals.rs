// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

impl Analyzer<'_> {
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
        // Preserve symbolic lengths until both sides are known. The shared
        // check decides whether the expected length can be retained.
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

    fn resolved_const_array_list_elem_type(
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
}
