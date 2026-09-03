// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ids::InternedTyId;
use nia_ty::TyKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClosureFunctionPointerCoercion {
    Compatible,
    Capturing,
    Mismatch,
}

impl BodyChecker<'_> {
    pub(crate) fn coerce_function_pointer_to_callable(
        &mut self,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        let expected = self.normalize_aliases_in_type(expected);
        let actual = self.normalize_aliases_in_type(actual);
        let Some(TyKind::Callable {
            params: expected_params,
            return_type: expected_return,
            ..
        }) = self.interner.get(expected).cloned()
        else {
            return None;
        };
        let Some(TyKind::FunctionPointer {
            params: actual_params,
            return_type: actual_return,
            is_variadic: false,
        }) = self.interner.get(actual).cloned()
        else {
            return None;
        };
        if expected_params.len() != actual_params.len()
            || !expected_params
                .iter()
                .zip(actual_params)
                .all(|(expected, actual)| self.types_match(*expected, actual))
            || !self.types_match(expected_return, actual_return)
        {
            return None;
        }
        Some(expected)
    }

    pub(crate) fn coerce_closure_pointer_to_callable(
        &mut self,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        let expected = self.normalize_aliases_in_type(expected);
        let actual = self.normalize_aliases_in_type(actual);
        let Some(TyKind::Callable {
            is_readonly: expected_readonly,
            params: expected_params,
            return_type: expected_return,
        }) = self.interner.get(expected).cloned()
        else {
            return None;
        };
        let Some(TyKind::Pointer {
            is_readonly: actual_readonly,
            elem,
        }) = self.interner.get(actual).cloned()
        else {
            return None;
        };
        let Some(TyKind::ClosureState {
            params: actual_params,
            return_type: actual_return,
            ..
        }) = self.interner.get(elem).cloned()
        else {
            return None;
        };

        if (!expected_readonly && actual_readonly)
            || expected_params.len() != actual_params.len()
            || !expected_params
                .iter()
                .zip(actual_params)
                .all(|(expected, actual)| self.types_match(*expected, actual))
            || !self.types_match(expected_return, actual_return)
        {
            return None;
        }
        Some(expected)
    }

    pub(crate) fn closure_to_function_pointer(
        &mut self,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> ClosureFunctionPointerCoercion {
        let expected = self.normalize_aliases_in_type(expected);
        let actual = self.normalize_aliases_in_type(actual);
        let Some(TyKind::FunctionPointer {
            params: expected_params,
            return_type: expected_return,
            is_variadic: false,
        }) = self.interner.get(expected).cloned()
        else {
            return ClosureFunctionPointerCoercion::Mismatch;
        };
        let Some(TyKind::ClosureState {
            captures,
            params: actual_params,
            return_type: actual_return,
            ..
        }) = self.interner.get(actual).cloned()
        else {
            return ClosureFunctionPointerCoercion::Mismatch;
        };
        if !captures.is_empty() {
            return ClosureFunctionPointerCoercion::Capturing;
        }
        if expected_params.len() == actual_params.len()
            && expected_params
                .iter()
                .zip(actual_params)
                .all(|(expected, actual)| self.types_match(*expected, actual))
            && self.types_match(expected_return, actual_return)
        {
            ClosureFunctionPointerCoercion::Compatible
        } else {
            ClosureFunctionPointerCoercion::Mismatch
        }
    }
}
