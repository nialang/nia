// SPDX-License-Identifier: GPL-3.0-or-later
//! Driver timing and query-observability counters.

use super::*;

pub(super) fn time_detail_stage<T>(timings: TimingMode, name: &str, f: impl FnOnce() -> T) -> T {
    nia_timing::time_stage(timings, nia_timing::TimingLevel::Detail, name, f)
}

pub(super) trait ProviderDemandOutput {
    fn checked_body_count(&self) -> usize;
    fn reachable_body_count(&self) -> usize;

    fn checked_module_count(&self) -> Option<usize> {
        None
    }

    fn monomorphized_instance_count(&self) -> Option<usize> {
        None
    }

    fn backend_module_count(&self) -> Option<usize> {
        None
    }

    fn backend_function_stats(&self) -> Option<BackendFunctionStats> {
        None
    }

    fn link_input_count(&self) -> Option<usize> {
        None
    }
}

pub(super) struct LiveCodegenCounters {
    pub(super) checked_body_count: usize,
    pub(super) reachable_body_count: usize,
    pub(super) backend_function_stats: BackendFunctionStats,
    pub(super) link_input_count: Option<usize>,
    pub(super) checked_module_count: usize,
    pub(super) monomorphized_instance_count: usize,
    pub(super) backend_module_count: usize,
}

impl ProviderDemandOutput for LiveCodegenCounters {
    fn checked_body_count(&self) -> usize {
        self.checked_body_count
    }

    fn reachable_body_count(&self) -> usize {
        self.reachable_body_count
    }

    fn checked_module_count(&self) -> Option<usize> {
        Some(self.checked_module_count)
    }

    fn monomorphized_instance_count(&self) -> Option<usize> {
        Some(self.monomorphized_instance_count)
    }

    fn backend_module_count(&self) -> Option<usize> {
        Some(self.backend_module_count)
    }

    fn backend_function_stats(&self) -> Option<BackendFunctionStats> {
        Some(self.backend_function_stats)
    }

    fn link_input_count(&self) -> Option<usize> {
        self.link_input_count
    }
}

impl ProviderDemandOutput for CheckedProgram {
    fn checked_body_count(&self) -> usize {
        self.checked_body_count()
    }

    fn reachable_body_count(&self) -> usize {
        self.reachable_body_count()
    }
}

#[cfg(test)]
impl ProviderDemandOutput for nia_compiler_query::CheckedProgramAnalysis {
    fn checked_body_count(&self) -> usize {
        self.modules
            .iter()
            .map(|module| module.body_ir.function_bodies.len())
            .sum()
    }

    fn reachable_body_count(&self) -> usize {
        self.checked_body_count()
    }
}

impl ProviderDemandOutput for CodegenProgram {
    fn checked_body_count(&self) -> usize {
        self.modules
            .iter()
            .map(|module| module.body_ir.function_bodies.len())
            .sum()
    }

    fn reachable_body_count(&self) -> usize {
        self.backend_function_stats()
            .expect("codegen programs have backend function counts")
            .definitions()
    }

    fn checked_module_count(&self) -> Option<usize> {
        Some(self.modules.len())
    }

    fn monomorphized_instance_count(&self) -> Option<usize> {
        Some(self.monomorphization.instances.len())
    }

    fn backend_module_count(&self) -> Option<usize> {
        Some(self.backend_lowering.program.modules.len())
    }

    fn backend_function_stats(&self) -> Option<BackendFunctionStats> {
        Some(self.backend_lowering.program.function_stats())
    }
}

pub(super) fn emit_compilation_counters(
    timings: TimingMode,
    database: &CompilerDatabase,
    loader_trace: &nia_query::QueryTrace,
    output: &impl ProviderDemandOutput,
    provider_demand_rounds: u64,
    source_stats: nia_source::SourceTableStats,
) -> nia_query::QueryResult<()> {
    if !timings.enabled() {
        return Ok(());
    }
    let compiler_trace = database.query_trace()?;
    let traces = [loader_trace, &compiler_trace];
    let graph = database.module_graph()?;
    let type_store_types = database.type_store().len() as u64;
    nia_timing::emit_counter("source.paths", source_stats.path_count);
    nia_timing::emit_counter("source.child_requests", source_stats.child_requests);
    nia_timing::emit_counter("source.child_hits", source_stats.child_hits);
    nia_timing::emit_counter("source.child_misses", source_stats.child_misses);
    nia_timing::emit_counter("compiler.loaded_modules", graph.modules().count() as u64);
    nia_timing::emit_counter("compiler.type_store_types", type_store_types);
    nia_timing::emit_counter(
        "compiler.type_store_growth",
        type_store_types.saturating_sub(1),
    );
    nia_timing::emit_counter(
        "compiler.semantic_selected_modules",
        graph
            .modules()
            .filter(|module| module.semantic_selected)
            .count() as u64,
    );
    if let Some(count) = output.checked_module_count() {
        nia_timing::emit_counter("compiler.checked_modules", count as u64);
    }
    if let Some(count) = output.monomorphized_instance_count() {
        nia_timing::emit_counter("compiler.monomorphized_instances", count as u64);
    }
    if let Some(count) = output.backend_module_count() {
        nia_timing::emit_counter("compiler.backend_modules", count as u64);
    }
    if let Some(stats) = output.backend_function_stats() {
        nia_timing::emit_counter(
            "compiler.backend_source_function_items",
            stats.source_items() as u64,
        );
        nia_timing::emit_counter(
            "compiler.backend_source_function_definitions",
            stats.source_definitions() as u64,
        );
        nia_timing::emit_counter(
            "compiler.backend_function_instance_items",
            stats.instance_items() as u64,
        );
        nia_timing::emit_counter(
            "compiler.backend_function_instance_definitions",
            stats.instance_definitions() as u64,
        );
    }
    if let Some(count) = output.link_input_count() {
        nia_timing::emit_counter("compiler.link_inputs", count as u64);
    }
    nia_timing::emit_counter(
        "query.executions",
        traces
            .iter()
            .flat_map(|trace| trace.queries.iter())
            .map(|query| query.stats.executions as u64)
            .sum(),
    );
    nia_timing::emit_counter(
        "query.cache_hits",
        traces
            .iter()
            .flat_map(|trace| trace.queries.iter())
            .map(|query| query.stats.cache_hits as u64)
            .sum(),
    );
    nia_timing::emit_counter(
        "query.waits",
        traces
            .iter()
            .flat_map(|trace| trace.queries.iter())
            .map(|query| query.stats.waits as u64)
            .sum(),
    );
    nia_timing::emit_counter(
        "query.validations",
        traces
            .iter()
            .flat_map(|trace| trace.queries.iter())
            .map(|query| query.stats.validations as u64)
            .sum(),
    );
    nia_timing::emit_counter(
        "query.green_validations",
        traces
            .iter()
            .flat_map(|trace| trace.queries.iter())
            .map(|query| query.stats.green_validations as u64)
            .sum(),
    );
    nia_timing::emit_counter(
        "query.slots",
        traces.iter().flat_map(|trace| trace.queries.iter()).count() as u64,
    );
    nia_timing::emit_counter(
        "query.dependency_edges",
        traces
            .iter()
            .map(|trace| trace.dependencies.len() as u64)
            .sum(),
    );
    let mut dependency_fanout = std::collections::HashMap::<&str, u64>::new();
    for dependency in traces.iter().flat_map(|trace| trace.dependencies.iter()) {
        *dependency_fanout
            .entry(dependency.from.description.as_ref())
            .or_default() += 1;
    }
    nia_timing::emit_counter(
        "query.max_dependency_fanout",
        dependency_fanout.values().copied().max().unwrap_or(0),
    );
    for (counter, query_name) in [
        ("query.executions.parsed_module", "parsed_module"),
        (
            "query.executions.loader_active_module_item_tree_fact",
            "loader_active_module_item_tree_fact",
        ),
        (
            "query.executions.module_declarations",
            "module_declarations",
        ),
        ("query.executions.provider_summary", "provider_summary"),
        (
            "query.executions.module_facade_facts",
            "module_facade_facts",
        ),
        (
            "query.executions.loader_public_surface_module_facts",
            "loader_public_surface_module_facts",
        ),
        ("query.executions.module_defs", "module_defs"),
        ("query.executions.full_module_defs", "full_module_defs"),
        (
            "query.executions.public_surface_module_facts",
            "public_surface_module_facts",
        ),
        ("query.executions.module_item_tree", "module_item_tree"),
        (
            "query.executions.active_module_item_tree",
            "active_module_item_tree",
        ),
        ("query.executions.type_resolution", "type_resolution"),
        ("query.executions.type_lowering", "type_lowering"),
        ("query.executions.item_signatures", "item_signatures"),
        (
            "query.executions.signature_type_resolution",
            "signature_type_resolution",
        ),
        (
            "query.executions.signature_type_lowering",
            "signature_type_lowering",
        ),
        (
            "query.executions.signature_item_signatures",
            "signature_item_signatures",
        ),
        (
            "query.executions.signature_type_normalization",
            "signature_type_normalization",
        ),
        (
            "query.executions.module_program_signature_facts",
            "module_program_signature_facts",
        ),
        (
            "query.executions.extension_signature_module_input",
            "extension_signature_module_input",
        ),
        (
            "query.executions.extension_trait_solving_module_facts",
            "extension_trait_solving_module_facts",
        ),
        (
            "query.executions.extension_provider_module_facts",
            "extension_provider_module_facts",
        ),
        (
            "query.executions.extension_provider_nominal_module_facts",
            "extension_provider_nominal_module_facts",
        ),
        (
            "query.executions.extension_provider_validation_facts",
            "extension_provider_validation_facts",
        ),
        ("query.executions.const_module", "const_module"),
        ("query.executions.const", "const"),
        (
            "query.executions.const_array_lengths",
            "const_array_lengths",
        ),
        ("query.executions.const_enum_values", "const_enum_values"),
        ("query.executions.const_values", "const_values"),
        ("query.executions.const_typed_facts", "const_typed_facts"),
        ("query.executions.layouts", "layouts"),
        ("query.executions.abi_check", "abi_check"),
        ("query.executions.static_check", "static_check"),
        ("query.executions.flow_check", "flow_check"),
        ("query.executions.body_check", "body_check"),
        (
            "query.executions.executable_checked_module_facts",
            "executable_checked_module_facts",
        ),
        (
            "query.executions.executable_checked_modules",
            "executable_checked_modules",
        ),
        (
            "query.executions.executable_value_ref_item",
            "executable_value_ref_item",
        ),
        (
            "query.executions.executable_value_ref_edges",
            "executable_value_ref_edges",
        ),
        (
            "query.executions.executable_function_body",
            "executable_function_body",
        ),
        (
            "query.executions.executable_static_init",
            "executable_static_init",
        ),
        (
            "query.executions.lowered_function_body",
            "lowered_function_body",
        ),
        ("query.executions.signature_layouts", "signature_layouts"),
        ("query.executions.type_normalization", "type_normalization"),
    ] {
        nia_timing::emit_counter(
            counter,
            traces
                .iter()
                .flat_map(|trace| trace.queries.iter())
                .filter(|query| query.frame.name == query_name)
                .map(|query| query.stats.executions as u64)
                .sum(),
        );
    }
    emit_query_category_counters(&traces);
    emit_query_validation_failure_counters(&traces);
    nia_timing::emit_counter("driver.provider_demand_rounds", provider_demand_rounds);
    nia_timing::emit_counter(
        "compiler.checked_bodies",
        output.checked_body_count() as u64,
    );
    nia_timing::emit_counter(
        "compiler.reachable_bodies",
        output.reachable_body_count() as u64,
    );
    Ok(())
}

fn emit_query_category_counters(traces: &[&nia_query::QueryTrace]) {
    let mut grouped = std::collections::BTreeMap::<(&str, &str), (u64, u64)>::new();
    for query in traces.iter().flat_map(|trace| trace.queries.iter()) {
        let Some(category) = query.frame.stats_category else {
            continue;
        };
        let (slots, executions) = grouped.entry((query.frame.name, category)).or_default();
        *slots += 1;
        *executions += query.stats.executions as u64;
    }
    for ((query_name, category), (slots, executions)) in grouped {
        nia_timing::emit_counter(format!("query.slots.{query_name}.{category}"), slots);
        nia_timing::emit_counter(
            format!("query.executions.{query_name}.{category}"),
            executions,
        );
        nia_timing::emit_counter(
            format!("query.reexecutions.{query_name}.{category}"),
            executions.saturating_sub(slots),
        );
    }
}

fn emit_query_validation_failure_counters(traces: &[&nia_query::QueryTrace]) {
    let mut grouped = std::collections::BTreeMap::<(&str, &str, &str, &str), u64>::new();
    for failure in traces
        .iter()
        .flat_map(|trace| trace.validation_failures.iter())
    {
        *grouped
            .entry((
                failure.query.name,
                failure.query.stats_category.unwrap_or("all"),
                failure.dependency.name,
                failure.reason.as_str(),
            ))
            .or_default() += failure.count as u64;
    }
    for ((query, category, dependency, reason), count) in grouped {
        nia_timing::emit_counter(
            format!("query.validation_failures.{query}.{category}.{dependency}.{reason}"),
            count,
        );
    }
}
