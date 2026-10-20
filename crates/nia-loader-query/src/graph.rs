#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ModuleGraphQuery;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ModuleGraphRevisionQuery(pub(crate) nia_loader_contract::ProviderFactRevision);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ModuleSourceIdQuery(pub(crate) nia_imports::ModuleId);

use crate::provider_facts::{ProviderDemandsQuery, ProviderFactEvent};
use crate::provider_loading::{
    add_public_reexport_source_module, module_defines_extensions, process_provider_request,
    process_reexport_provider_request,
};
use crate::queries::module_declarations_query;
use crate::used_paths::{UsedModulePath, UsedModulePathProcessing};
use crate::{LoaderContext, RuntimeSpec, runtime_package_root_path};
use nia_diagnostic::Diagnostic;
use nia_imports::{
    ModuleGraph, ModuleGraphError, ModuleGraphSnapshot, ModuleNode, ModulePath,
    ResolvedModuleDeclaration, module_declaration_visibility_allows,
};
use nia_loader_contract::{ProgramDiagnostic, ProgramDiagnosticBundles};
use nia_query::{QueryDb, QueryError, QueryFingerprintPolicy, QueryKey, QueryResult};
use nia_source::{SourceId, SourcePath};
use nia_symbol::SymbolId;
use std::collections::HashSet;

#[derive(Debug)]
pub(crate) enum TraversalError {
    Query(QueryError),
    Diagnostic(Diagnostic),
}

impl From<QueryError> for TraversalError {
    fn from(error: QueryError) -> Self {
        Self::Query(error)
    }
}

impl From<Diagnostic> for TraversalError {
    fn from(diagnostic: Diagnostic) -> Self {
        Self::Diagnostic(diagnostic)
    }
}

impl From<nia_ice::Ice> for TraversalError {
    fn from(ice: nia_ice::Ice) -> Self {
        Self::Query(ice.into())
    }
}

impl From<ModuleGraphError> for TraversalError {
    fn from(error: ModuleGraphError) -> Self {
        match error {
            ModuleGraphError::Diagnostic(diagnostic) => Self::Diagnostic(diagnostic),
            ModuleGraphError::Internal(ice) => Self::Query(ice.into()),
        }
    }
}

pub(crate) type TraversalResult<T> = Result<T, TraversalError>;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModuleGraphValue {
    pub(crate) semantic: ModuleGraphSnapshot,
    pub(crate) diagnostics: ProgramDiagnosticBundles,
}

impl QueryKey<LoaderContext> for ModuleGraphQuery {
    type Value = ModuleGraphValue;

    fn name() -> &'static str {
        "module_graph"
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        let provider_facts = db.get(ProviderDemandsQuery)?;
        Ok(db
            .get(ModuleGraphRevisionQuery(provider_facts.revision()))?
            .as_ref()
            .clone())
    }
}

impl QueryKey<LoaderContext> for ModuleGraphRevisionQuery {
    type Value = ModuleGraphValue;

    fn name() -> &'static str {
        "module_graph_revision"
    }

    fn description(&self) -> String {
        format!("module_graph_revision({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        let event = db.context().provider_facts.event(self.0).ok_or_else(|| {
            db.invalid_input(self, format!("unknown provider fact revision {:?}", self.0))
        })?;
        let graph = match event {
            ProviderFactEvent::Current { demands } => build_module_graph(db, None, &demands),
            ProviderFactEvent::Added { previous, demands } => {
                let seed = db.get(ModuleGraphRevisionQuery(previous))?;
                build_module_graph(db, Some(seed), &demands)
            }
        }?;
        db.context().provider_facts.compact_transition(self.0);
        Ok(graph)
    }
}

impl QueryKey<LoaderContext> for ModuleSourceIdQuery {
    type Value = Option<SourceId>;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "module_source_id"
    }

    fn description(&self) -> String {
        format!("module_source_id({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        let graph = db.get(ModuleGraphQuery)?;
        Ok(graph.semantic.get(self.0).map(|module| module.source_id))
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

fn build_module_graph(
    db: &QueryDb<LoaderContext>,
    seed: Option<std::sync::Arc<ModuleGraphValue>>,
    new_provider_demands: &std::collections::HashSet<nia_loader_contract::ProviderDemand>,
) -> QueryResult<ModuleGraphValue> {
    db.context()
        .runtime
        .validate_for_target(&db.context().target)
        .map_err(|error| db.invalid_input(&ModuleGraphQuery, error.to_string()))?;
    let mut fresh_diagnostics = Vec::new();
    let mut pending_trait_provider_imports = Vec::new();
    let (mut graph, prior_diagnostics, mut index) = match seed {
        Some(value) => {
            let mut graph = (*value.semantic).clone();
            let existing_modules = graph.modules().count();
            for demand in new_provider_demands {
                match &demand.request {
                    nia_loader_contract::ProviderRequest::ModuleSemantic { module_path } => {
                        record_traversal_diagnostic(
                            activate_semantic_provider_module(db, &mut graph, module_path),
                            &mut fresh_diagnostics,
                            module_path,
                        )?;
                    }
                    nia_loader_contract::ProviderRequest::ModuleBody { module_path } => {
                        if let Some(module_id) =
                            graph.module_id_for_source_identity(&module_path.identity())
                        {
                            record_traversal_diagnostic(
                                mark_process_used_paths_and_process(db, &mut graph, module_id),
                                &mut fresh_diagnostics,
                                module_path,
                            )?;
                        }
                    }
                    nia_loader_contract::ProviderRequest::Method { .. }
                    | nia_loader_contract::ProviderRequest::TraitImpl { .. } => {}
                }
            }
            let existing_nodes = graph
                .modules()
                .take(existing_modules)
                .cloned()
                .collect::<Vec<_>>();
            for node in existing_nodes {
                let declarations = db.get(module_declarations_query(db, &node.path)?)?;
                apply_provider_demands(
                    db,
                    &mut graph,
                    &node,
                    &declarations.semantic.explicit_imports,
                    new_provider_demands,
                    &mut fresh_diagnostics,
                    &mut pending_trait_provider_imports,
                )?;
            }
            process_pending_trait_provider_imports(
                db,
                &mut graph,
                &mut pending_trait_provider_imports,
                &mut fresh_diagnostics,
            )?;
            (graph, value.diagnostics.clone(), existing_modules)
        }
        None => {
            let source_table = db.context().sources.source_table();
            let mut graph = match &db.context().package_root {
                Some(package_root) => ModuleGraph::with_package_root_and_source_table(
                    db.context().entry_path.clone(),
                    package_root.clone(),
                    std::sync::Arc::new(db.context().symbols.clone()),
                    source_table,
                ),
                None => ModuleGraph::with_source_table(
                    db.context().entry_path.clone(),
                    std::sync::Arc::new(db.context().symbols.clone()),
                    source_table,
                ),
            }?;
            inject_entry_runtime(db, &mut graph)?;
            (
                graph,
                ProgramDiagnosticBundles::from_diagnostics_in(
                    db.context().diagnostic_store.clone(),
                    Vec::new(),
                )?,
                0,
            )
        }
    };
    loop {
        let node = graph.modules().nth(index).cloned();
        let Some(node) = node else {
            break;
        };
        let declarations = db.get(module_declarations_query(db, &node.path)?)?;
        mount_module_packages(db, &mut graph, &node, &declarations.semantic.package_roots)?;
        if should_eager_add_declarations(db.context(), &node) {
            record_traversal_diagnostic(
                add_declared_module_children(db, &mut graph, node.id),
                &mut fresh_diagnostics,
                &node.path,
            )?;
        }
        if should_process_used_module_paths(db.context(), &node) {
            for path in ordered_used_module_paths(&declarations.semantic.used_module_paths) {
                record_traversal_diagnostic(
                    add_used_module_path(db, &mut graph, node.id, &path),
                    &mut fresh_diagnostics,
                    &node.path,
                )?;
            }
        }
        apply_provider_demands(
            db,
            &mut graph,
            &node,
            &declarations.semantic.explicit_imports,
            new_provider_demands,
            &mut fresh_diagnostics,
            &mut pending_trait_provider_imports,
        )?;
        process_pending_trait_provider_imports(
            db,
            &mut graph,
            &mut pending_trait_provider_imports,
            &mut fresh_diagnostics,
        )?;
        index += 1;
    }
    let diagnostics = ProgramDiagnosticBundles::from_diagnostics_in(
        db.context().diagnostic_store.clone(),
        fresh_diagnostics
            .into_iter()
            .map(|(path, diagnostic)| ProgramDiagnostic { path, diagnostic })
            .collect(),
    )?;
    Ok(ModuleGraphValue {
        semantic: ModuleGraphSnapshot::new(graph),
        diagnostics: prior_diagnostics.append(&diagnostics)?,
    })
}

fn mount_module_packages(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    node: &ModuleNode,
    packages: &[SymbolId],
) -> QueryResult<()> {
    let map = &db.context().module_map;
    let scoped = map.has_scope(&node.path);
    if scoped {
        graph.isolate_package_roots(node.id);
    }
    for name in packages {
        let Some((instance, path)) = map.binding_for_source(&node.path, *name) else {
            continue;
        };
        let root = graph.intern_package_root(&instance, path.clone())?;
        if scoped {
            graph.bind_package_alias(node.id, *name, root);
        }
    }
    Ok(())
}

fn process_pending_trait_provider_imports(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    pending: &mut Vec<nia_imports::ModuleId>,
    diagnostics: &mut Vec<(SourcePath, Diagnostic)>,
) -> QueryResult<()> {
    for provider in pending.drain(..) {
        if let Some(provider_node) = graph.get(provider).cloned() {
            record_traversal_diagnostic(
                process_trait_provider_imports(db, graph, provider),
                diagnostics,
                &provider_node.path,
            )?;
        }
    }
    Ok(())
}

/// Selects the provider's direct semantic imports without promoting its whole
/// import closure. Trait witnesses contribute facts to the requesting module;
/// their ordinary extension scope remains lazy and local to the provider.
fn process_trait_provider_imports(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    module_id: nia_imports::ModuleId,
) -> TraversalResult<()> {
    if !graph.mark_semantic_selected(module_id) {
        return Ok(());
    }
    let Some(node) = graph.get(module_id).cloned() else {
        return Ok(());
    };
    let declarations = db.get(module_declarations_query(db, &node.path)?)?;
    mount_module_packages(db, graph, &node, &declarations.semantic.package_roots)?;
    for path in ordered_used_module_paths(&declarations.semantic.used_module_paths) {
        if matches!(
            path.processing(),
            UsedModulePathProcessing::IfSelectedItem
                | UsedModulePathProcessing::IfProvidesExtensions
                | UsedModulePathProcessing::IfProvidesTraitMethod { .. }
        ) {
            continue;
        }
        let Some(start) = used_path_start(graph, module_id, &path) else {
            continue;
        };
        if let Some(package) = path.activates_package_facade() {
            activate_package_facade(db, graph, module_id, package)?;
        }
        let shallow_path = path.with_appended_segments_with_processing_mode(
            &[],
            path.include_declared_children(),
            UsedModulePathProcessing::Never,
        );
        let Some(imported) = add_visible_declared_module_path(
            db,
            graph,
            module_id,
            start,
            shallow_path.segments(),
            shallow_path.processing(),
        )?
        else {
            continue;
        };
        if shallow_path.include_declared_children() {
            add_declared_module_children(db, graph, imported)?;
        }
    }
    Ok(())
}

fn record_traversal_diagnostic(
    result: TraversalResult<()>,
    diagnostics: &mut Vec<(SourcePath, Diagnostic)>,
    path: &SourcePath,
) -> QueryResult<()> {
    match result {
        Ok(()) => Ok(()),
        Err(TraversalError::Query(error)) => Err(error),
        Err(TraversalError::Diagnostic(diagnostic)) => {
            diagnostics.push((path.clone(), diagnostic));
            Ok(())
        }
    }
}

fn activate_semantic_provider_module(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    module_path: &SourcePath,
) -> TraversalResult<()> {
    let Some(module_id) = graph.module_id_for_source_identity(&module_path.identity()) else {
        return Ok(());
    };
    mark_semantic_used_paths_and_process(db, graph, module_id)
}

fn mark_semantic_used_paths_and_process(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    module_id: nia_imports::ModuleId,
) -> TraversalResult<()> {
    if !graph.mark_semantic_selected(module_id) {
        return Ok(());
    }
    let Some(node) = graph.get(module_id).cloned() else {
        return Ok(());
    };
    process_semantic_module_dependencies(db, graph, &node)
}

/// Processes a selected module's semantic dependencies without promoting it
/// to the eager source-processing state. Provider paths may need declarations
/// from a module whose own `using` graph must remain lazy.
fn process_semantic_module_dependencies(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    node: &ModuleNode,
) -> TraversalResult<()> {
    let declarations = db.get(module_declarations_query(db, &node.path)?)?;
    mount_module_packages(db, graph, node, &declarations.semantic.package_roots)?;
    for path in ordered_used_module_paths(&declarations.semantic.used_module_paths) {
        add_semantic_used_module_path(db, graph, node.id, &path)?;
    }
    Ok(())
}

fn add_semantic_used_module_path(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    current_module: nia_imports::ModuleId,
    path: &UsedModulePath,
) -> TraversalResult<()> {
    if matches!(
        path.processing(),
        UsedModulePathProcessing::IfProvidesExtensions
            | UsedModulePathProcessing::IfProvidesTraitMethod { .. }
    ) {
        return add_used_module_path(db, graph, current_module, path);
    }
    if path.processing() == UsedModulePathProcessing::Never {
        return Ok(());
    }
    let Some(start) = used_path_start(graph, current_module, path) else {
        return Ok(());
    };
    if let Some(package) = path.activates_package_facade() {
        activate_package_facade(db, graph, current_module, package)?;
    }
    let shallow_path = path.with_appended_segments_with_processing_mode(
        &[],
        path.include_declared_children(),
        UsedModulePathProcessing::Never,
    );
    let Some(module_id) = add_visible_declared_module_path(
        db,
        graph,
        current_module,
        start,
        shallow_path.segments(),
        shallow_path.processing(),
    )?
    else {
        return Ok(());
    };
    if shallow_path.include_declared_children() {
        add_declared_module_children(db, graph, module_id)?;
    }
    mark_semantic_used_paths_and_process(db, graph, module_id)
}

fn apply_provider_demands(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    node: &ModuleNode,
    imports: &[crate::used_paths::ExplicitUsingImport],
    provider_demands: &std::collections::HashSet<nia_loader_contract::ProviderDemand>,
    diagnostics: &mut Vec<(SourcePath, Diagnostic)>,
    pending_trait_provider_imports: &mut Vec<nia_imports::ModuleId>,
) -> QueryResult<()> {
    let mut demands = provider_demands
        .iter()
        .filter(|demand| demand.source_path.identity() == node.path.identity())
        .cloned()
        .collect::<Vec<_>>();
    demands.sort_unstable_by_key(|demand| match &demand.request {
        nia_loader_contract::ProviderRequest::ModuleSemantic { .. }
        | nia_loader_contract::ProviderRequest::ModuleBody { .. } => 0,
        nia_loader_contract::ProviderRequest::Method {
            target_type_name: Some(_),
            ..
        }
        | nia_loader_contract::ProviderRequest::TraitImpl {
            target_type_name: Some(_),
            ..
        } => 1,
        nia_loader_contract::ProviderRequest::Method {
            target_type_name: None,
            ..
        }
        | nia_loader_contract::ProviderRequest::TraitImpl {
            target_type_name: None,
            ..
        } => 2,
    });
    for demand in demands {
        if let nia_loader_contract::ProviderRequest::ModuleSemantic { module_path } = demand.request
        {
            record_traversal_diagnostic(
                activate_semantic_provider_module(db, graph, &module_path),
                diagnostics,
                &module_path,
            )?;
            continue;
        }
        if let nia_loader_contract::ProviderRequest::TraitImpl { .. } = &demand.request {
            apply_trait_demand(
                db,
                graph,
                node,
                &demand.request,
                diagnostics,
                pending_trait_provider_imports,
            )?;

            // Trait implementations are package-owned facts. Once the two
            // owner packages have been searched, `using` paths must not add a
            // third package to the candidate set. Ordinary method demands
            // still use the importing module's extension scope below.
            continue;
        }
        for import in imports {
            let processing = match demand.request {
                nia_loader_contract::ProviderRequest::Method {
                    target_type_name,
                    method_name,
                } => UsedModulePathProcessing::IfProvidesTraitMethod {
                    target_type_name,
                    associated_name: method_name,
                },
                nia_loader_contract::ProviderRequest::ModuleSemantic { .. } => continue,
                nia_loader_contract::ProviderRequest::ModuleBody { .. } => continue,
                nia_loader_contract::ProviderRequest::TraitImpl { .. } => continue,
            };
            let path =
                import
                    .path
                    .with_appended_segments_with_processing_mode(&[], false, processing);
            record_traversal_diagnostic(
                add_used_module_path(db, graph, node.id, &path),
                diagnostics,
                &node.path,
            )?;
        }
    }
    Ok(())
}

fn apply_trait_demand(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    requesting_node: &ModuleNode,
    request: &nia_loader_contract::ProviderRequest,
    diagnostics: &mut Vec<(SourcePath, Diagnostic)>,
    pending_trait_provider_imports: &mut Vec<nia_imports::ModuleId>,
) -> QueryResult<()> {
    let nia_loader_contract::ProviderRequest::TraitImpl {
        target_type_name,
        target_package,
        target_module_segments,
        trait_name,
        trait_package,
        trait_type_argument_names,
    } = request
    else {
        return Ok(());
    };
    let mut visited = HashSet::new();
    let mut providers = Vec::new();
    let mut packages = Vec::new();
    if let Some(package) = target_package {
        packages.push(*package);
    }
    if let Some(package) = trait_package {
        packages.push(*package);
    }
    packages.sort();
    packages.dedup();
    for package in packages {
        if graph.package_root(&package).is_none() {
            mount_module_packages(db, graph, requesting_node, &[package])?;
        }
        let Some(root_id) = graph
            .package_root(&package)
            .or_else(|| graph.root_module_for_name(requesting_node.id, package))
        else {
            continue;
        };
        let Some(root) = graph.get(root_id).cloned() else {
            continue;
        };
        let package = root.module_path.package;
        TraitProviderSearch {
            db,
            graph,
            package,
            target_type_name: *target_type_name,
            target_module_segments,
            trait_name: *trait_name,
            trait_type_argument_names,
            visited: &mut visited,
            providers: &mut providers,
        }
        .collect(&root.path, &root.module_path, &[])?;
    }
    for (package, segments) in providers {
        record_traversal_diagnostic(
            add_package_trait_provider(
                db,
                graph,
                requesting_node.id,
                package,
                &segments,
                pending_trait_provider_imports,
            ),
            diagnostics,
            &requesting_node.path,
        )?;
    }
    Ok(())
}

fn add_package_trait_provider(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    requesting_module: nia_imports::ModuleId,
    package: SymbolId,
    segments: &[SymbolId],
    pending_trait_provider_imports: &mut Vec<nia_imports::ModuleId>,
) -> TraversalResult<()> {
    let Some(mut current) = graph.package_root(&package) else {
        return Ok(());
    };
    for segment in segments {
        // Implementation discovery follows ownership, not the caller's name
        // visibility. Each declaration is accessed from its own parent.
        let Some(next) = add_visible_declared_module_child_if_present(
            db, graph, current, current, segment, false,
        )?
        else {
            return Ok(());
        };
        current = next;
    }
    graph.add_trait_impl_dependency(requesting_module, current);
    if !pending_trait_provider_imports.contains(&current) {
        pending_trait_provider_imports.push(current);
    }
    Ok(())
}

struct TraitProviderSearch<'a> {
    db: &'a QueryDb<LoaderContext>,
    graph: &'a ModuleGraph,
    package: SymbolId,
    target_type_name: Option<SymbolId>,
    target_module_segments: &'a [SymbolId],
    trait_name: SymbolId,
    trait_type_argument_names: &'a [Option<SymbolId>],
    visited: &'a mut HashSet<String>,
    providers: &'a mut Vec<(SymbolId, Vec<SymbolId>)>,
}

impl TraitProviderSearch<'_> {
    fn collect(
        &mut self,
        parent_path: &SourcePath,
        parent_module_path: &ModulePath,
        segments: &[SymbolId],
    ) -> QueryResult<()> {
        // Package-owned discovery can walk a large standard or dependency
        // package. Keep the traversal worklist based so module depth cannot
        // consume the query worker's stack.
        let mut pending = vec![(
            parent_path.clone(),
            parent_module_path.clone(),
            segments.to_vec(),
        )];
        while let Some((path, module_path, path_segments)) = pending.pop() {
            let summary = self
                .db
                .get(crate::queries::provider_summary_query(self.db, &path)?)?;
            if summary.defines_trait_impl_at_module(
                self.target_type_name.as_ref(),
                self.target_module_segments,
                &module_path.segments,
                &self.trait_name,
                self.trait_type_argument_names,
                None,
            ) {
                self.providers.push((self.package, path_segments.clone()));
            }
            let declarations = self.db.get(module_declarations_query(self.db, &path)?)?;
            for declaration in declarations.semantic.declarations.iter().rev() {
                let child_path = self.graph.declared_child_source_path_for(
                    &path,
                    &module_path,
                    declaration.name,
                );
                let child_module_path = module_path.child(declaration.name);
                let key = child_path.identity().normalized_path().to_owned();
                if !self.visited.insert(key) {
                    continue;
                }
                let mut child_segments = path_segments.clone();
                child_segments.push(declaration.name);
                pending.push((child_path, child_module_path, child_segments));
            }
        }
        Ok(())
    }
}

fn should_eager_add_declarations(context: &LoaderContext, node: &ModuleNode) -> bool {
    node.process_declared_children
        || (node.module_path.is_package_root()
            && context
                .package_roots_with_used_paths
                .contains(&node.module_path.package))
        || node.module_path.is_runtime_start_module()
}

fn should_process_used_module_paths(context: &LoaderContext, node: &ModuleNode) -> bool {
    node.process_used_paths
        || (node.module_path.is_package_root()
            && context
                .package_roots_with_used_paths
                .contains(&node.module_path.package))
}

fn add_used_module_path(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    current_module: nia_imports::ModuleId,
    path: &UsedModulePath,
) -> TraversalResult<()> {
    let Some(start) = used_path_start(graph, current_module, path) else {
        return Ok(());
    };
    if let Some(package) = path.activates_package_facade() {
        activate_package_facade(db, graph, current_module, package)?;
    }
    let Some(module_id) = add_visible_declared_module_path(
        db,
        graph,
        current_module,
        start,
        path.segments(),
        path.processing(),
    )?
    else {
        return Ok(());
    };
    if path.include_declared_children() {
        add_declared_module_children(db, graph, module_id)?;
    }
    Ok(())
}

fn activate_package_facade(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    current_module: nia_imports::ModuleId,
    package: SymbolId,
) -> TraversalResult<()> {
    let Some(package) = graph
        .root_module_for_name(current_module, package)
        .and_then(|root| graph.get(root))
        .map(|node| node.module_path.package)
    else {
        return Ok(());
    };
    if graph.package_facade_active(&package) {
        return Ok(());
    }
    let Some(root) = graph.mark_package_facade_active(&package) else {
        return Ok(());
    };
    let Some(node) = graph.get(root).cloned() else {
        return Ok(());
    };
    let declarations = db.get(module_declarations_query(db, &node.path)?)?;
    mount_module_packages(db, graph, &node, &declarations.semantic.package_roots)?;
    for path in ordered_used_module_paths(&declarations.semantic.used_module_paths) {
        add_used_module_path(db, graph, root, &path)?;
    }
    Ok(())
}

pub(crate) fn used_path_start(
    graph: &ModuleGraph,
    current_module: nia_imports::ModuleId,
    path: &UsedModulePath,
) -> Option<nia_imports::ModuleId> {
    match path {
        UsedModulePath::Package { package, .. } => {
            graph.root_module_for_name(current_module, *package)
        }
        UsedModulePath::PackageRelative { .. } => graph.current_package_root(current_module),
        UsedModulePath::ParentRelative { .. } => {
            graph.get(current_module).and_then(|node| node.parent)
        }
        UsedModulePath::Local { .. } => Some(current_module),
    }
}

pub(crate) fn mark_process_used_paths_and_process(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    module_id: nia_imports::ModuleId,
) -> TraversalResult<()> {
    if !graph.mark_process_used_paths(module_id) {
        return Ok(());
    }
    let Some(node) = graph.get(module_id).cloned() else {
        return Ok(());
    };
    let declarations = db.get(module_declarations_query(db, &node.path)?)?;
    mount_module_packages(db, graph, &node, &declarations.semantic.package_roots)?;
    for path in ordered_used_module_paths(&declarations.semantic.used_module_paths) {
        add_used_module_path(db, graph, module_id, &path)?;
    }
    Ok(())
}

fn ordered_used_module_paths(paths: &[UsedModulePath]) -> Vec<UsedModulePath> {
    let mut ordered = paths.to_vec();
    ordered.sort_unstable_by_key(|path| match path.processing() {
        UsedModulePathProcessing::Never | UsedModulePathProcessing::Always => 0_u8,
        UsedModulePathProcessing::IfSelectedItem => 1,
        UsedModulePathProcessing::IfProvidesExtensions => 2,
        UsedModulePathProcessing::IfProvidesTraitMethod { .. } => 3,
    });
    ordered
}

pub(crate) fn add_visible_declared_module_path(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    accessing_module: nia_imports::ModuleId,
    start: nia_imports::ModuleId,
    segments: &[SymbolId],
    processing: UsedModulePathProcessing,
) -> TraversalResult<Option<nia_imports::ModuleId>> {
    add_visible_declared_module_path_following(
        db,
        graph,
        accessing_module,
        start,
        segments,
        processing,
        &mut Vec::new(),
    )
}

/// Re-export facades whose source path is being followed, innermost last.
///
/// A `pub using` source can name the re-exported item itself (`pub using x;`
/// or a wildcard), and facades can re-export each other. Following a facade
/// name that is already in progress cannot discover a new module, so the
/// traversal stops there and leaves the unresolved name to public-surface
/// diagnostics.
pub(crate) type ReexportStack = Vec<(nia_imports::ModuleId, SymbolId)>;

pub(crate) fn add_visible_declared_module_path_following(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    accessing_module: nia_imports::ModuleId,
    start: nia_imports::ModuleId,
    segments: &[SymbolId],
    processing: UsedModulePathProcessing,
    reexports: &mut ReexportStack,
) -> TraversalResult<Option<nia_imports::ModuleId>> {
    let mut current = start;
    if processing == UsedModulePathProcessing::Always && segments.is_empty() {
        mark_process_used_paths_and_process(db, graph, current)?;
    }
    if segments.is_empty() {
        match processing {
            UsedModulePathProcessing::IfSelectedItem => {
                mark_process_used_paths_and_process(db, graph, current)?;
            }
            UsedModulePathProcessing::IfProvidesExtensions
                if module_defines_extensions(db, graph, current)? =>
            {
                mark_process_used_paths_and_process(db, graph, current)?;
            }
            _ => {}
        }
    }
    if segments.is_empty() {
        process_provider_request(db, graph, accessing_module, current, &processing)?;
    }
    for (index, segment) in segments.iter().enumerate() {
        let is_terminal = index + 1 == segments.len();
        let process_segment_used_paths =
            processing == UsedModulePathProcessing::Always && is_terminal;
        let Some(next) = add_visible_declared_module_child_if_present(
            db,
            graph,
            accessing_module,
            current,
            segment,
            process_segment_used_paths,
        )?
        else {
            let reexport_facade = current;
            let source_processing = (processing == UsedModulePathProcessing::Never)
                .then_some(UsedModulePathProcessing::Never);
            let Some(reexport_source) = add_public_reexport_source_module(
                db,
                graph,
                current,
                segment,
                source_processing,
                reexports,
            )?
            else {
                match processing {
                    UsedModulePathProcessing::Always => {
                        mark_process_used_paths_and_process(db, graph, current)?;
                    }
                    UsedModulePathProcessing::IfSelectedItem => {
                        mark_semantic_used_paths_and_process(db, graph, current)?;
                    }
                    _ => {}
                }
                return Ok(Some(current));
            };
            if processing == UsedModulePathProcessing::Always && !is_terminal {
                mark_process_used_paths_and_process(db, graph, current)?;
            }
            process_reexport_provider_request(
                db,
                graph,
                accessing_module,
                reexport_facade,
                segment,
                &processing,
            )?;
            current = reexport_source;
            if matches!(
                processing,
                UsedModulePathProcessing::Always | UsedModulePathProcessing::IfSelectedItem
            ) && is_terminal
            {
                mark_process_used_paths_and_process(db, graph, current)?;
            }
            if is_terminal {
                process_provider_request(db, graph, accessing_module, current, &processing)?;
            }
            continue;
        };
        current = next;
        if processing.is_provider_demand() {
            process_provider_request(db, graph, accessing_module, current, &processing)?;
        }
        if processing == UsedModulePathProcessing::IfSelectedItem
            && is_terminal
            && module_defines_extensions(db, graph, current)?
        {
            mark_process_used_paths_and_process(db, graph, current)?;
        }
        if processing == UsedModulePathProcessing::IfProvidesExtensions
            && is_terminal
            && module_defines_extensions(db, graph, current)?
        {
            mark_process_used_paths_and_process(db, graph, current)?;
        }
        if is_terminal && !processing.is_provider_demand() {
            process_provider_request(db, graph, accessing_module, current, &processing)?;
        }
    }
    Ok(Some(current))
}

fn add_declared_module_children(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    module_id: nia_imports::ModuleId,
) -> TraversalResult<()> {
    let Some(node) = graph.get(module_id).cloned() else {
        return Ok(());
    };
    let declarations = db.get(module_declarations_query(db, &node.path)?)?;
    for declaration in declarations.semantic.declarations.iter().cloned() {
        add_declared_module_child(db, graph, module_id, declaration)?;
    }
    Ok(())
}

pub(crate) fn add_visible_declared_module_child_if_present(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    accessing_module: nia_imports::ModuleId,
    module_id: nia_imports::ModuleId,
    name: &SymbolId,
    process_used_paths: bool,
) -> TraversalResult<Option<nia_imports::ModuleId>> {
    if let Some(existing) = graph
        .get(module_id)
        .and_then(|node| node.children.get(name).copied())
    {
        if process_used_paths {
            mark_process_used_paths_and_process(db, graph, existing)?;
        }
        return Ok(Some(existing));
    }
    let Some(node) = graph.get(module_id).cloned() else {
        return Ok(None);
    };
    let declarations = db.get(module_declarations_query(db, &node.path)?)?;
    let Some(declaration) = declarations
        .semantic
        .declarations
        .iter()
        .find(|declaration| {
            declaration.name == *name
                && module_declaration_visibility_allows(
                    declaration.visibility,
                    graph,
                    module_id,
                    accessing_module,
                )
        })
        .cloned()
    else {
        return Ok(None);
    };
    add_declared_module_child_with_processing(
        db,
        graph,
        module_id,
        declaration,
        process_used_paths,
        false,
    )
    .map(Some)
}

fn add_declared_module_child(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    module_id: nia_imports::ModuleId,
    declaration: ResolvedModuleDeclaration,
) -> TraversalResult<nia_imports::ModuleId> {
    add_declared_module_child_with_processing(db, graph, module_id, declaration, true, true)
}

fn add_declared_module_child_with_processing(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    module_id: nia_imports::ModuleId,
    declaration: ResolvedModuleDeclaration,
    process_used_paths: bool,
    process_declared_children: bool,
) -> TraversalResult<nia_imports::ModuleId> {
    if let Some(existing) = graph
        .get(module_id)
        .and_then(|node| node.children.get(&declaration.name).copied())
    {
        if process_used_paths {
            mark_process_used_paths_and_process(db, graph, existing)?;
        }
        if process_declared_children {
            graph.mark_process_declared_children(existing);
        }
        return Ok(existing);
    }
    Ok(graph.intern_declared_child_with_processing(
        module_id,
        &declaration.name,
        declaration.visibility,
        declaration.span,
        process_used_paths,
        process_declared_children,
    )?)
}

fn inject_entry_runtime(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
) -> nia_ice::IceResult<()> {
    match &db.context().runtime {
        RuntimeSpec::Bare => {}
        RuntimeSpec::Source(runtime) => {
            let runtime_root_path = runtime_package_root_path(runtime);
            let runtime_root = graph.intern_runtime_package_root(runtime_root_path)?;
            graph.mark_process_declared_children(runtime_root);
            graph.mark_executable_root_subtree(runtime_root);
        }
    }
    Ok(())
}
