// SPDX-License-Identifier: GPL-3.0-or-later
use nia_diagnostic::Diagnostic;
use nia_ids::GlobalDefId;
use nia_span::Span;

use super::{BackendValidator, FunctionInstanceRef, backend_symbol_debug_name};

impl BackendValidator<'_> {
    pub(super) fn validate_function_ref(&mut self, def_id: GlobalDefId, span: Span, message: &str) {
        if !self.index.has_function(def_id) {
            let module_name = self
                .index
                .module(def_id.module_id)
                .map(|module| module.name.as_str())
                .unwrap_or("<missing module>");
            let current_item = self.current_item.as_deref().unwrap_or("<unknown item>");
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!("{message} {def_id:?} in `{module_name}` while validating {current_item}"),
            ));
        }
    }

    pub(super) fn validate_function_instance_ref(
        &mut self,
        instance: FunctionInstanceRef<'_>,
        span: Span,
        message: &str,
    ) {
        let FunctionInstanceRef {
            def_id,
            arg_module_id,
            self_arg,
            args,
            const_args,
        } = instance;
        if let Some(self_arg) = self_arg {
            self.validate_type(self_arg, span);
        }
        for arg in args {
            self.validate_type(*arg, span);
        }
        let key = (
            def_id,
            arg_module_id,
            self_arg,
            args.to_vec(),
            const_args.to_vec(),
        );
        let exists = if let Some(exists) = self.function_instance_ref_cache.borrow().get(&key) {
            *exists
        } else {
            let exists = self
                .index
                .function_instance(def_id, arg_module_id, self_arg, args, const_args)
                .is_some()
                || self.index.function_instances_for(def_id).any(|item| {
                    self.same_optional_type(item.self_arg, self_arg)
                        && self.same_type_args(&item.args, args)
                        && item.const_args.as_slice() == const_args
                });
            self.function_instance_ref_cache
                .borrow_mut()
                .insert(key, exists);
            exists
        };
        if !exists {
            let available_instances = self.index.function_instance_count(def_id);
            let function_name = self
                .index
                .function(def_id)
                .map(|function| backend_symbol_debug_name(function.name))
                .unwrap_or_else(|| "<missing template>".to_string());
            let module_name = self
                .index
                .module(def_id.module_id)
                .map(|module| module.name.as_str())
                .unwrap_or("<missing module>");
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!(
                    "{message} {def_id:?} `{function_name}` in `{module_name}` with arg_module_id {arg_module_id:?}, {} args; {available_instances} instances exist for this def",
                    args.len(),
                ),
            ));
        }
    }
}
