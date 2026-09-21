// SPDX-License-Identifier: GPL-3.0-or-later
//! Compiler query database ownership and session construction.

use super::*;

/// Incremental compiler database bound to one loader/query session.
#[derive(Clone)]
pub struct CompilerDatabase {
    pub(super) db: QueryDb<CompilerContext>,
    pub(super) inputs: Arc<RwLock<CompilerInputs>>,
}

impl std::fmt::Debug for CompilerDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inputs = self.inputs.read();
        f.debug_struct("CompilerDatabase")
            .field("optimization", &inputs.optimization)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompilerInvalidation {
    pub invalidated: Vec<QueryFrame>,
}

impl CompilerInvalidation {
    pub(super) fn extend(&mut self, invalidation: nia_query::QueryInvalidation) {
        for frame in invalidation.invalidated {
            if !self.invalidated.contains(&frame) {
                self.invalidated.push(frame);
            }
        }
    }
}

pub(super) fn compiler_database_with_providers(
    request: CompileRequest,
    providers: CompilerQueryProviders,
) -> QueryResult<CompilerDatabase> {
    let session = match request.loader_facts.query_session() {
        Some(session) => session,
        None => nia_query::QuerySession::new()?,
    };
    compiler_database_with_providers_in_session(request, providers, session)
}

pub(super) fn compiler_database_with_providers_in_session(
    request: CompileRequest,
    providers: CompilerQueryProviders,
    session: nia_query::QuerySession,
) -> QueryResult<CompilerDatabase> {
    let timings = request.timings;
    let signature_cache = request.frontend_cache_dir.as_ref().map(|root| {
        Arc::new(crate::signature_cache::PersistentSignatureCache::new(
            root.clone(),
        ))
    });
    let verify_frontend_cache = request.verify_frontend_cache;
    let loader_facts = Arc::clone(&request.loader_facts);
    let observed_graph = loader_facts.module_graph()?;
    if let Some(loader_session) = loader_facts.query_session()
        && !session.ptr_eq(&loader_session)
    {
        return Err(QueryError::internal(
            "compiler and loader facts must share one query session",
        ));
    }
    let node_store = loader_facts.node_store();
    let inputs = Arc::new(RwLock::new(CompilerInputs::new(request)));
    let executable_fact_session = Arc::new(Mutex::new(ExecutableFactSession::default()));
    let type_store = Arc::new(nia_ty::TypeStore::new()?);
    let db = QueryDb::new_registered_with_timings_in_session(
        CompilerContext {
            inputs: inputs.clone(),
            observed_graph: Mutex::new(observed_graph),
            loader_facts,
            providers,
            executable_fact_session,
            executable_fact_scheduler: Mutex::new(()),
            type_store,
            diagnostic_store: nia_diagnostic::DiagnosticStore::new()?,
            node_store,
            signature_cache,
            verify_frontend_cache,
            provider_settlement_scheduler: Mutex::new(()),
            frontend_cache_publications: Mutex::new(None),
            provider_demand_rounds: std::sync::atomic::AtomicU64::new(0),
        },
        timings,
        compiler_query_registry()?,
        session,
    )?;
    Ok(CompilerDatabase { db, inputs })
}
