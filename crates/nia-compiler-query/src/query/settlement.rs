// SPDX-License-Identifier: GPL-3.0-or-later
//! Provider-demand fixed-point settlement for compiler products.

use super::*;

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
