// SPDX-License-Identifier: GPL-3.0-or-later
//! Function-local storage type validation contracts.

use super::*;

impl BackendValidator<'_> {
    pub(super) fn validate_local_type(
        &mut self,
        local_id: nia_ids::LocalId,
        actual_ty: nia_ids::InternedTyId,
        span: Span,
        message: &'static str,
    ) {
        let Some(expected_ty) = self
            .local_tys
            .last()
            .and_then(|locals| locals.get(&local_id))
            .copied()
        else {
            return;
        };
        if !self.same_type(expected_ty, actual_ty) {
            self.invalid_local_type(span, message);
        }
    }

    pub(super) fn invalid_local_type(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR contains an invalid local type contract: {message}"),
        ));
    }
}
