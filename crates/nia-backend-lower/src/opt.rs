// SPDX-License-Identifier: GPL-3.0-or-later
use crate::ModuleLowerer;
use nia_function_ir::FunctionBody;
use nia_function_opt::{FunctionOptInput, optimize_function_body};
use nia_ids::GlobalDefId;
use nia_opt::OptimizationPolicy;

pub(crate) fn enabled_function_passes(policy: &OptimizationPolicy) -> Vec<&'static str> {
    nia_function_opt::enabled_function_passes(policy)
}

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn optimize_function_body(
        &mut self,
        function: GlobalDefId,
        is_instance: bool,
        type_arg_count: usize,
        body: FunctionBody,
    ) -> FunctionBody {
        let zero_sized_types = |ty| self.layout_of(ty).is_some_and(|layout| layout.size == 0);
        let output = optimize_function_body(FunctionOptInput {
            body,
            policy: &self.optimization,
            is_zero_sized: zero_sized_types,
        });
        if let Some(error) = output.validation_error {
            self.diagnostics
                .push(nia_diagnostic::Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    error.span,
                    format!("function optimizer rejected invalid IR: {}", error.message),
                ));
        }
        for pass in output.changed_passes {
            self.optimization_report.changed_passes.push(
                crate::BackendOptimizationChange::Function {
                    module_id: self.input.module_id,
                    function,
                    pass,
                    is_instance,
                    type_arg_count,
                },
            );
        }
        output.body
    }
}
