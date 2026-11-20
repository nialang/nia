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
        let (graph_changed, handle_generation_changed) = {
            let observed = self.db.context().observed_graph.lock();
            let changed = !observed.ptr_eq(&new_graph) && *observed != new_graph;
            let timings = request.timings;
            let handles_changed = if changed && timings.detail() {
                let mut removed = 0;
                let mut reassigned = 0;
                for old in observed.modules() {
                    match new_graph.module_id_for_stable_key(&old.stable_key) {
                        None => removed += 1,
                        Some(id) if id != old.id => reassigned += 1,
                        Some(_) => {}
                    }
                }
                let added = new_graph
                    .modules()
                    .filter(|new| observed.module_id_for_stable_key(&new.stable_key).is_none())
                    .count();
                for (change, count) in [
                    ("removed", removed),
                    ("reassigned", reassigned),
                    ("added", added),
                ] {
                    if count != 0 {
                        nia_timing::emit_counter(
                            format!("compiler.module_graph_handles.{change}"),
                            count as u64,
                        );
                    }
                }
                removed != 0 || reassigned != 0
            } else {
                changed
                    && observed.modules().any(|old| {
                        let Some(key) = observed.stable_key(old.id) else {
                            return false;
                        };
                        new_graph
                            .module_id_for_stable_key(key)
                            .is_none_or(|new| new != old.id)
                    })
            };
            (changed, handles_changed)
        };
        let new_inputs = CompilerInputs::new(request);
        let (optimization_changed, root_scope_changed, current_package_changed) = {
            let mut inputs = self.inputs.write();
            let optimization_changed = inputs.optimization != new_inputs.optimization;
            let root_scope_changed = inputs.root_scope != new_inputs.root_scope;
            let current_package_changed = inputs.current_package != new_inputs.current_package;
            *inputs = new_inputs;
            (
                optimization_changed,
                root_scope_changed,
                current_package_changed,
            )
        };
        let mut invalidated = Vec::new();
        if graph_changed {
            // The executable fact epoch contains session-local module handles;
            // a graph replacement makes that value and every dependent red.
            invalidated.extend(self.db.invalidate(ExecutableFactEpochQuery)?.invalidated);
            if handle_generation_changed {
                self.db
                    .retire_scope(|frame| frame.name == "loaded_modules")?;
            }
            let loaded_modules = StableModuleSequence::from_source_identities(
                self.db
                    .context()
                    .loader_facts()
                    .loaded_module_source_identities()?,
            );
            invalidated.extend(
                self.db
                    .validate_input(LoadedModulesQuery, &loaded_modules)?
                    .invalidated,
            );
            if handle_generation_changed {
                if self.db.context().timings().enabled() {
                    nia_timing::emit_counter(
                        "compiler.executable_fact_session.module_handle_reset",
                        1,
                    );
                }
                *self.db.context().executable_fact_session.lock() =
                    ExecutableFactSession::default();
            }
        }
        self.invalidate_inputs(
            &mut invalidated,
            optimization_changed,
            root_scope_changed,
            current_package_changed,
        )?;
        if graph_changed {
            *self.db.context().observed_graph.lock() = new_graph;
        }
        let mut seen = HashSet::with_capacity(invalidated.len());
        invalidated.retain(|frame| seen.insert(frame.clone()));
        Ok(CompilerInvalidation { invalidated })
    }
    fn invalidate_inputs(
        &self,
        invalidated: &mut Vec<QueryFrame>,
        optimization_changed: bool,
        root_scope_changed: bool,
        current_package_changed: bool,
    ) -> QueryResult<()> {
        let provider_worklist = self.db.context().provider_fact_worklist()?;
        invalidated.extend(
            self.db
                .validate_input(ProviderFactWorklistQuery, &provider_worklist)?
                .invalidated,
        );
        if optimization_changed {
            invalidated.extend(self.db.invalidate(CompilerOptimizationQuery)?.invalidated);
        }
        if root_scope_changed {
            invalidated.extend(self.db.invalidate(CompilerRootScopeQuery)?.invalidated);
        }
        if current_package_changed {
            invalidated.extend(self.db.invalidate(MonomorphizationQuery)?.invalidated);
            invalidated.extend(self.db.invalidate(BackendLoweringInputsQuery)?.invalidated);
        }
        Ok(())
    }
}
