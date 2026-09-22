// SPDX-License-Identifier: GPL-3.0-or-later
//! Compiler session update and query invalidation façade.

use super::*;

impl CompilerDatabase {
    /// Replaces session-compatible inputs and returns the resulting invalidation set.
    ///
    /// The loader session, frontend cache root, and verification policy cannot
    /// change in place because they own persisted and in-memory query identity.
    pub fn update(&self, request: CompileRequest) -> QueryResult<CompilerInvalidation> {
        let loader_session = request.loader_facts.query_session().ok_or_else(|| {
            QueryError::internal("compiler updates require a tracked loader fact provider")
        })?;
        if !self.db.session().ptr_eq(&loader_session) {
            return Err(QueryError::internal(
                "compiler update loader facts belong to a different query session",
            ));
        }
        if request.frontend_cache_dir.as_deref()
            != self
                .db
                .context()
                .signature_cache
                .as_ref()
                .map(|cache| cache.root())
        {
            return Err(QueryError::internal(
                "compiler frontend cache root cannot change within a query session",
            ));
        }
        if request.verify_frontend_cache != self.db.context().verify_frontend_cache {
            return Err(QueryError::internal(
                "compiler frontend cache verification cannot change within a query session",
            ));
        }
        let new_graph = request.loader_facts.module_graph()?;
        let graph_changed = {
            let observed = self.db.context().observed_graph.lock();
            *observed != new_graph
        };
        let handle_generation_changed = {
            let observed = self.db.context().observed_graph.lock();
            observed.modules().any(|old| {
                let Some(key) = observed.stable_key(old.id) else {
                    return false;
                };
                new_graph
                    .module_id_for_stable_key(key)
                    .is_none_or(|new| new != old.id)
            })
        };
        let new_inputs = CompilerInputs::new(request);
        let (optimization_changed, codegen_scope_changed, current_package_changed) = {
            let mut inputs = self.inputs.write();
            let optimization_changed = inputs.optimization != new_inputs.optimization;
            let codegen_scope_changed = inputs.codegen_scope != new_inputs.codegen_scope;
            let current_package_changed = inputs.current_package != new_inputs.current_package;
            *inputs = new_inputs;
            (
                optimization_changed,
                codegen_scope_changed,
                current_package_changed,
            )
        };
        let mut invalidation = CompilerInvalidation::default();
        if graph_changed {
            // The executable fact epoch contains session-local module handles;
            // a graph replacement makes that value and every dependent red.
            invalidation.extend(self.db.invalidate(ExecutableFactEpochQuery)?);
            if handle_generation_changed {
                self.db
                    .session()
                    .invalidate_scope(|frame| frame.name == "loaded_modules")?;
            }
            let loaded_modules = StableModuleSequence::from_source_identities(
                self.db
                    .context()
                    .loader_facts()
                    .loaded_module_source_identities()?,
            );
            invalidation.extend(
                self.db
                    .validate_input(LoadedModulesQuery, &loaded_modules)?,
            );
            if handle_generation_changed {
                *self.db.context().executable_fact_session.lock() =
                    ExecutableFactSession::default();
            }
        }
        let inputs_invalidation = self.invalidate_inputs(
            optimization_changed,
            codegen_scope_changed,
            current_package_changed,
        )?;
        invalidation
            .invalidated
            .extend(inputs_invalidation.invalidated);
        if graph_changed {
            *self.db.context().observed_graph.lock() = new_graph;
        }
        Ok(invalidation)
    }
    fn invalidate_inputs(
        &self,
        optimization_changed: bool,
        codegen_scope_changed: bool,
        current_package_changed: bool,
    ) -> QueryResult<CompilerInvalidation> {
        let mut invalidation = CompilerInvalidation::default();
        let provider_worklist = self.db.context().provider_fact_worklist()?;
        invalidation.extend(
            self.db
                .validate_input(ProviderFactWorklistQuery, &provider_worklist)?,
        );
        if optimization_changed {
            invalidation.extend(self.db.invalidate(CompilerOptimizationQuery)?);
        }
        if codegen_scope_changed {
            invalidation.extend(self.db.invalidate(CompilerCodegenScopeQuery)?);
        }
        if current_package_changed {
            invalidation.extend(self.db.invalidate(MonomorphizationQuery)?);
            invalidation.extend(self.db.invalidate(BackendLoweringInputsQuery)?);
        }
        Ok(invalidation)
    }
}
