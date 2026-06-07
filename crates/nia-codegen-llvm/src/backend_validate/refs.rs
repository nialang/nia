// SPDX-License-Identifier: GPL-3.0-or-later
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_span::Span;

use super::BackendValidator;

impl BackendValidator<'_> {
    pub(super) fn validate_function_ref(&mut self, def_id: GlobalDefId, span: Span, message: &str) {
        if !self.index.functions.contains_key(&def_id) {
            self.diagnostics.push(Diagnostic::internal_error_at(
                "I0300",
                span,
                format!("{message} {def_id:?}"),
            ));
        }
    }

    pub(super) fn validate_function_instance_ref(
        &mut self,
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        args: &[InternedTyId],
        span: Span,
        message: &str,
    ) {
        for arg in args {
            self.validate_type(*arg, span);
        }
        let key = (def_id, arg_module_id, args.to_vec());
        let exists = if let Some(exists) = self.function_instance_ref_cache.borrow().get(&key) {
            *exists
        } else {
            let exists = self
                .index
                .function_instance(def_id, arg_module_id, args)
                .is_some()
                || self
                    .index
                    .function_instances_by_def
                    .get(&def_id)
                    .into_iter()
                    .flatten()
                    .any(|item| self.same_type_args(&item.args, args));
            self.function_instance_ref_cache
                .borrow_mut()
                .insert(key, exists);
            exists
        };
        if !exists {
            let available_instances = self
                .index
                .function_instances_by_def
                .get(&def_id)
                .map_or(0, |instances| instances.len());
            let function_name = self
                .index
                .functions
                .get(&def_id)
                .map(|function| function.name.as_str())
                .unwrap_or("<missing template>");
            let module_name = self
                .index
                .modules
                .get(&def_id.module_id)
                .map(|module| module.name.as_str())
                .unwrap_or("<missing module>");
            self.diagnostics.push(Diagnostic::internal_error_at(
                "I0300",
                span,
                format!(
                    "{message} {def_id:?} `{function_name}` in `{module_name}` with arg_module_id {arg_module_id:?}, {} args; {available_instances} instances exist for this def",
                    args.len(),
                ),
            ));
        }
    }
}
