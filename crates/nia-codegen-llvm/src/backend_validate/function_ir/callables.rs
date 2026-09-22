// SPDX-License-Identifier: GPL-3.0-or-later
//! Closure state and callable ABI validation contracts.

use super::*;

impl BackendValidator<'_> {
    pub(super) fn validate_callable_coercion(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        state: &FunctionExpr,
        closure_id: nia_ids::ClosureId,
        span: Span,
    ) {
        self.validate_expr(state);
        let Some(TyKind::Callable {
            is_readonly: callable_readonly,
            params: callable_params,
            return_type: callable_return,
        }) = self.ty_kind(result_ty).cloned()
        else {
            self.invalid_callable_coercion(span, "result is not callable");
            return;
        };
        let Some(TyKind::Pointer {
            is_readonly: state_readonly,
            elem: state_ty,
        }) = self.ty_kind(state.ty).cloned()
        else {
            self.invalid_callable_coercion(span, "state is not a pointer");
            return;
        };
        let Some(TyKind::ClosureState {
            closure_id: state_closure_id,
            params: state_params,
            return_type: state_return,
            ..
        }) = self.ty_kind(state_ty).cloned()
        else {
            self.invalid_callable_coercion(span, "state pointer does not target closure state");
            return;
        };
        if state_closure_id != closure_id {
            self.invalid_callable_coercion(span, "closure identity does not match its state");
        }
        if !callable_readonly && state_readonly {
            self.invalid_callable_coercion(span, "mutable callable has a readonly state pointer");
        }
        if !self.same_type_args(&callable_params, &state_params)
            || !self.same_type(callable_return, state_return)
        {
            self.invalid_callable_coercion(span, "callable signature does not match closure state");
        }
        let Some(entry) = self.current_closure_entry(closure_id) else {
            self.invalid_callable_coercion(span, "generated closure entry is missing");
            return;
        };
        if !self.same_type(entry.abi.state_type, state_ty)
            || !self.same_type_args(&entry.abi.params, &callable_params)
            || !self.same_type(entry.abi.return_type, callable_return)
        {
            self.invalid_callable_coercion(span, "generated entry ABI does not match the callable");
        }
    }

    pub(super) fn validate_closure_function_pointer(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        closure_id: nia_ids::ClosureId,
        span: Span,
    ) {
        let Some(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic: false,
        }) = self.ty_kind(result_ty).cloned()
        else {
            self.invalid_callable_coercion(
                span,
                "closure function-pointer result is not a non-variadic function pointer",
            );
            return;
        };
        let Some(entry) = self.current_closure_entry(closure_id) else {
            self.invalid_callable_coercion(span, "generated closure entry is missing");
            return;
        };
        let state_contract = match self.ty_kind(entry.abi.state_type) {
            Some(TyKind::ClosureState {
                closure_id: state_closure_id,
                captures,
                params: state_params,
                return_type: state_return,
            }) => {
                *state_closure_id == closure_id
                    && captures.is_empty()
                    && self.same_type_args(state_params, &params)
                    && self.same_type(*state_return, return_type)
            }
            _ => false,
        };
        if !state_contract
            || !self.same_type_args(&entry.abi.params, &params)
            || !self.same_type(entry.abi.return_type, return_type)
        {
            self.invalid_callable_coercion(
                span,
                "closure entry is capturing or has a mismatched function-pointer ABI",
            );
        }
    }

    fn current_closure_entry(
        &self,
        closure_id: nia_ids::ClosureId,
    ) -> Option<&nia_backend_ir::BackendClosureEntry> {
        let owner = self.current_closure_owner.clone()?;
        self.index
            .closure_entry(&nia_backend_ir::BackendClosureEntryKey { closure_id, owner })
    }

    pub(super) fn invalid_callable_coercion(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR callable coercion has an invalid contract: {message}"),
        ));
    }
}
