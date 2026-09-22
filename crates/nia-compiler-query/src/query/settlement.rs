// SPDX-License-Identifier: GPL-3.0-or-later
//! Provider-demand fixed-point settlement for compiler products.

use super::*;

pub(super) fn emit_provider_graph_change(
    timings: TimingMode,
    round: u64,
    invalidates_resolved_body_facts: bool,
) {
    if !timings.enabled() {
        return;
    }
    let prefix = format!("compiler.executable_provider_demands.round_{round}");
    nia_timing::emit_counter(format!("{prefix}.graph_changed"), 1);
    nia_timing::emit_counter(
        format!("{prefix}.invalidates_body_facts"),
        u64::from(invalidates_resolved_body_facts),
    );
}

pub(super) fn emit_provider_demand_batch(
    timings: TimingMode,
    round: u64,
    demands: &[crate::ProviderDemand],
) {
    if !timings.enabled() {
        return;
    }
    let mut methods = 0_u64;
    let mut trait_impls = 0_u64;
    let mut module_semantics = 0_u64;
    let mut module_bodies = 0_u64;
    for demand in demands {
        match demand.request {
            crate::ProviderRequest::Method { .. } => methods += 1,
            crate::ProviderRequest::TraitImpl { .. } => trait_impls += 1,
            crate::ProviderRequest::ModuleSemantic { .. } => module_semantics += 1,
            crate::ProviderRequest::ModuleBody { .. } => module_bodies += 1,
        }
    }
    let prefix = format!("compiler.executable_provider_demands.round_{round}");
    nia_timing::emit_counter(format!("{prefix}.total"), demands.len() as u64);
    nia_timing::emit_counter(format!("{prefix}.methods"), methods);
    nia_timing::emit_counter(format!("{prefix}.trait_impls"), trait_impls);
    nia_timing::emit_counter(format!("{prefix}.module_semantics"), module_semantics);
    nia_timing::emit_counter(format!("{prefix}.module_bodies"), module_bodies);
}

pub(super) fn emit_provider_demand_update(
    timings: TimingMode,
    round: u64,
    phase: &str,
    unique: u64,
    known: u64,
    added: &[&crate::ProviderDemand],
) {
    if !timings.enabled() {
        return;
    }
    let prefix = format!("compiler.executable_provider_demands.round_{round}.{phase}");
    nia_timing::emit_counter(format!("{prefix}.unique"), unique);
    nia_timing::emit_counter(format!("{prefix}.known"), known);
    nia_timing::emit_counter(format!("{prefix}.new"), added.len() as u64);
    let mut methods = 0_u64;
    let mut trait_impls = 0_u64;
    let mut module_semantics = 0_u64;
    let mut module_bodies = 0_u64;
    for demand in added {
        match demand.request {
            crate::ProviderRequest::Method { .. } => methods += 1,
            crate::ProviderRequest::TraitImpl { .. } => trait_impls += 1,
            crate::ProviderRequest::ModuleSemantic { .. } => module_semantics += 1,
            crate::ProviderRequest::ModuleBody { .. } => module_bodies += 1,
        }
    }
    nia_timing::emit_counter(format!("{prefix}.new_methods"), methods);
    nia_timing::emit_counter(format!("{prefix}.new_trait_impls"), trait_impls);
    nia_timing::emit_counter(format!("{prefix}.new_module_semantics"), module_semantics);
    nia_timing::emit_counter(format!("{prefix}.new_module_bodies"), module_bodies);
}

pub(super) fn emit_check_certificate_reuse(timings: TimingMode, hit: bool) {
    if !timings.enabled() {
        return;
    }
    nia_timing::emit_counter("compiler.check_certificate_hits", u64::from(hit));
    nia_timing::emit_counter("compiler.check_certificate_misses", u64::from(!hit));
}

pub(super) fn checked_provider_demands(
    program: &CheckedProgramAnalysis,
) -> Vec<crate::ProviderDemand> {
    let mut demands = program
        .modules
        .iter()
        .flat_map(|module| module.provider_demands.iter().cloned())
        .collect::<Vec<_>>();
    demands.extend(program.modules.iter().filter_map(|module| {
        let needs_body_activation = program
            .graph
            .get(module.id)
            .is_some_and(|node| !node.process_used_paths);
        needs_body_activation.then(|| crate::ProviderDemand {
            source_path: module.path.clone(),
            request: crate::ProviderRequest::ModuleBody {
                module_path: module.path.clone(),
            },
        })
    }));
    demands
}

pub(super) fn codegen_provider_demands(program: &CodegenProgram) -> Vec<crate::ProviderDemand> {
    program
        .modules
        .iter()
        .flat_map(|module| module.provider_demands.iter().cloned())
        .collect()
}

pub(super) fn codegen_preparation_provider_demands(
    preparation: &CodegenPreparation,
) -> Vec<crate::ProviderDemand> {
    preparation
        .modules
        .iter()
        .flat_map(|module| module.provider_demands.iter().cloned())
        .collect()
}

pub(super) fn settle_provider_worklist<T>(
    database: &CompilerDatabase,
    discover_executable_providers: bool,
    compile: impl Fn(&CompilerDatabase) -> QueryResult<T>,
    provider_demands: impl Fn(&T) -> Vec<crate::ProviderDemand>,
) -> QueryResult<T> {
    // The loader fixed point and its deferred cache publications form one
    // session; concurrent top-level settlements must not mix either state.
    let _settlement = database.db.context().provider_settlement_scheduler.lock();
    database.db.context().begin_frontend_cache_publications()?;
    database.refresh_frontend_program_sources_snapshot()?;
    let result = (|| {
        let mut skip_executable_discovery = false;
        let mut rounds = 0_u64;
        loop {
            rounds += 1;
            database.refresh_frontend_program_sources_snapshot()?;
            if discover_executable_providers && !skip_executable_discovery {
                let timings = database.db.context().timings();
                let demands = nia_timing::time_query(
                    timings,
                    &format!("executable_provider_demands.round_{rounds}"),
                    || database.executable_provider_demands(),
                )?;
                emit_provider_demand_batch(database.db.context().timings(), rounds, &demands);
                if let crate::ProviderGraphUpdate::Changed {
                    invalidates_resolved_body_facts,
                } =
                    database.update_provider_demands_with_telemetry(rounds, "discovery", demands)?
                {
                    emit_provider_graph_change(
                        database.db.context().timings(),
                        rounds,
                        invalidates_resolved_body_facts,
                    );
                    skip_executable_discovery = !invalidates_resolved_body_facts;
                    continue;
                }
            }
            let output = compile(database)?;
            match database.update_provider_demands_with_telemetry(
                rounds,
                "compile",
                provider_demands(&output),
            )? {
                crate::ProviderGraphUpdate::Changed {
                    invalidates_resolved_body_facts,
                } => {
                    skip_executable_discovery =
                        discover_executable_providers && !invalidates_resolved_body_facts;
                }
                crate::ProviderGraphUpdate::Stable => {
                    database
                        .db
                        .context()
                        .loader_facts()
                        .settle_provider_demands()?;
                    database
                        .db
                        .context()
                        .provider_demand_rounds
                        .store(rounds, std::sync::atomic::Ordering::Relaxed);
                    return Ok(output);
                }
            }
        }
    })();
    let publications = database.db.context().finish_frontend_cache_publications();
    result.inspect(|_| {
        nia_timing::time_query(
            database.db.context().timings(),
            "frontend.signature_reuse_flush",
            || database.flush_frontend_cache_publications(publications),
        );
    })
}
