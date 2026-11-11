// SPDX-License-Identifier: GPL-3.0-or-later
//! `@[inlineAlways]` on a function with CPU features is a promise about
//! direct calls. Its definition does not carry LLVM's `alwaysinline`, which
//! would also force calls through pointers, whose callers need not have the
//! features. Each direct call whose caller has every required feature carries
//! the attribute instead.

use super::ModuleCodegen;
use nia_backend_ir::BackendFunctionAttribute;
use nia_diagnostic::Diagnostic;
use nia_llvm::values::FunctionValue;

impl<'ctx> ModuleCodegen<'ctx, '_> {
    /// Applies a declared function's attributes and records the features a
    /// direct call must have to inline it.
    pub(super) fn declare_function_attributes(
        &mut self,
        function: FunctionValue<'ctx>,
        attributes: &[BackendFunctionAttribute],
    ) -> Result<(), Diagnostic> {
        let required = nia_backend_ir::function_cpu_features(attributes);
        if attributes.contains(&BackendFunctionAttribute::InlineAlways)
            && required != nia_target::CpuFeatures::default()
        {
            self.feature_inline_functions.insert(function, required);
        }
        self.apply_function_attributes(function, attributes)
    }

    /// Whether a direct call from `owner` to `callee` must inline at this call
    /// site: the callee is feature-specific `@[inlineAlways]`, and the caller
    /// has the callee's features through its own attribute or the build.
    pub(crate) fn direct_call_inlines(
        &self,
        callee: FunctionValue<'ctx>,
        owner: &nia_backend_ir::BackendClosureEntryOwner,
    ) -> bool {
        let Some(&required) = self.feature_inline_functions.get(&callee) else {
            return false;
        };
        let attributes = match owner {
            nia_backend_ir::BackendClosureEntryOwner::Source(def_id) => self
                .program
                .function(*def_id)
                .map(|function| function.attributes.as_slice()),
            nia_backend_ir::BackendClosureEntryOwner::FunctionInstance(key) => self
                .program
                .function_instance(
                    key.def_id,
                    key.arg_module_id,
                    key.self_arg,
                    &key.args,
                    &key.const_args,
                )
                .map(|function| function.attributes.as_slice()),
        }
        .unwrap_or_default();
        required.is_subset_of(
            nia_backend_ir::function_cpu_features(attributes).union(self.build_cpu_features),
        )
    }
}
