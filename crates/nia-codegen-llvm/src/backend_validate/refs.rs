// SPDX-License-Identifier: GPL-3.0-or-later
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId};
use nia_span::Span;

use super::BackendValidator;

impl BackendValidator<'_> {
    pub(super) fn validate_function_ref(&mut self, def_id: GlobalDefId, span: Span, message: &str) {
        if !self.index.functions.contains_key(&def_id) {
            self.diagnostics
                .push(Diagnostic::error(span, format!("{message} {def_id:?}")));
        }
    }

    pub(super) fn validate_function_instance_ref(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        span: Span,
        message: &str,
    ) {
        for arg in args {
            self.validate_type(*arg, span);
        }
        let exists = self
            .index
            .function_instances_by_def
            .get(&def_id)
            .into_iter()
            .flatten()
            .any(|item| self.same_type_args(&item.args, args));
        if !exists {
            self.diagnostics
                .push(Diagnostic::error(span, format!("{message} {def_id:?}")));
        }
    }
}
