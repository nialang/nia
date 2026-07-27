// SPDX-License-Identifier: GPL-3.0-or-later
use nia_query::{QueryDb, QueryKey, QueryTrace};
use nia_symbol::{SymbolId, stable_hash};
use std::sync::Arc;

pub(super) trait QueryDbTestExt<C> {
    fn expect_get<K>(&self, key: K) -> Arc<K::Value>
    where
        K: QueryKey<C>;
}

impl<C> QueryDbTestExt<C> for QueryDb<C> {
    fn expect_get<K>(&self, key: K) -> Arc<K::Value>
    where
        K: QueryKey<C>,
    {
        self.get(key).expect("test query must succeed")
    }
}

thread_local! {
    static TEST_SYMBOLS: nia_symbol_table::SymbolTable = nia_symbol_table::SymbolTable::new();
}

pub(super) fn test_symbols() -> nia_symbol_table::SymbolTable {
    TEST_SYMBOLS.with(Clone::clone)
}

pub(super) fn sym(text: &str) -> SymbolId {
    test_symbols()
        .intern(text)
        .unwrap_or_else(|err| panic!("test symbol collision: {err}"));
    SymbolId::from_stable_hash(stable_hash(text))
}

pub(super) fn query_executions(trace: &QueryTrace, name: &'static str) -> usize {
    trace
        .queries
        .iter()
        .filter(|query| query.frame.name == name)
        .map(|query| query.stats.executions)
        .sum()
}

pub(super) fn query_cache_hits(trace: &QueryTrace, name: &'static str) -> usize {
    trace
        .queries
        .iter()
        .filter(|query| query.frame.name == name)
        .map(|query| query.stats.cache_hits)
        .sum()
}

pub(super) fn query_green_validations(trace: &QueryTrace, name: &'static str) -> usize {
    trace
        .queries
        .iter()
        .filter(|query| query.frame.name == name)
        .map(|query| query.stats.green_validations)
        .sum()
}

fn is_body_signature_query(name: &str) -> bool {
    matches!(name, "program_body_function_signatures")
}

pub(super) fn trace_has_dependency(trace: &QueryTrace, from: &str, to: &str) -> bool {
    trace
        .dependencies
        .iter()
        .any(|dependency| dependency.from.name == from && dependency.to.name == to)
}

pub(super) fn depends_on_body_signature_query(trace: &QueryTrace, from: &str) -> bool {
    trace.dependencies.iter().any(|dependency| {
        dependency.from.name == from && is_body_signature_query(dependency.to.name)
    })
}

pub(super) fn assert_query_executions_unchanged(
    before: &QueryTrace,
    after: &QueryTrace,
    name: &'static str,
) {
    assert_eq!(
        query_executions(before, name),
        query_executions(after, name),
        "{name} should have been reused"
    );
}
