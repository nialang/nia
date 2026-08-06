#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ModuleGraphQuery;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ModuleGraphRevisionQuery(pub(crate) nia_compiler_query::ProviderFactRevision);

use crate::provider_facts::{ProviderDemandsQuery, ProviderFactEvent};
use crate::provider_loading::{
    add_public_reexport_source_module, module_defines_extensions, process_provider_request,
    process_reexport_provider_request,
};
use crate::queries::module_declarations_query;
use crate::used_paths::{UsedModulePath, UsedModulePathProcessing};
use crate::{EntryRuntime, LoaderContext};
use nia_compiler_query::{ProgramDiagnostic, ProgramDiagnosticBundles};
use nia_diagnostic::Diagnostic;
use nia_imports::{
    ModuleGraph, ModuleGraphSnapshot, ModuleNode, ResolvedModuleDeclaration,
    module_declaration_visibility_allows,
};
use nia_query::{QueryDb, QueryError, QueryKey, QueryResult};
use nia_source::SourcePath;
use nia_span::Span;
use nia_symbol::{SymbolId, known};

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

fn build_module_graph(
    db: &QueryDb<LoaderContext>,
    seed: Option<std::sync::Arc<ModuleGraphValue>>,
    new_provider_demands: &std::collections::HashSet<nia_compiler_query::ProviderDemand>,
) -> QueryResult<ModuleGraphValue> {
    let mut fresh_diagnostics = Vec::new();
    let (mut graph, prior_diagnostics, mut index) = match seed {
        Some(value) => {
            let mut graph = (*value.semantic).clone();
            let existing_modules = graph.modules().count();
            for demand in new_provider_demands {
                match &demand.request {
                    nia_compiler_query::ProviderRequest::ModuleSemantic { module_path } => {
                        record_traversal_diagnostic(
                            activate_semantic_provider_module(db, &mut graph, module_path),
                            &mut fresh_diagnostics,
                            module_path,
                        )?;
                    }
                    nia_compiler_query::ProviderRequest::ModuleBody { module_path } => {
                        if let Some(module_id) = graph.module_id_for_path(module_path.as_str()) {
                            record_traversal_diagnostic(
                                mark_process_used_paths_and_process(db, &mut graph, module_id),
                                &mut fresh_diagnostics,
                                module_path,
                            )?;
                        }
                    }
                    nia_compiler_query::ProviderRequest::Method { .. }
                    | nia_compiler_query::ProviderRequest::TraitImpl { .. } => {}
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
                )?;
            }
            (graph, value.diagnostics.clone(), existing_modules)
        }
        None => {
            let mut graph = ModuleGraph::with_symbol_text(
                db.context().entry_path.clone(),
                std::sync::Arc::new(db.context().symbols.clone()),
            );
            inject_entry_runtime(db, &mut graph, &mut fresh_diagnostics);
            (
                graph,
                ProgramDiagnosticBundles::from_diagnostics_in(
                    db.context().diagnostic_store.clone(),
                    Vec::new(),
                ),
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
        for package in &declarations.semantic.package_roots {
            if graph.package_root(package).is_none()
                && let Some(path) = db.context().module_map.get_name(package)
            {
                graph.intern_package_root(package, path.clone());
            }
        }
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
        )?;
        index += 1;
    }
    let diagnostics = ProgramDiagnosticBundles::from_diagnostics_in(
        db.context().diagnostic_store.clone(),
        fresh_diagnostics
            .into_iter()
            .map(|(path, diagnostic)| ProgramDiagnostic { path, diagnostic })
            .collect(),
    );
    Ok(ModuleGraphValue {
        semantic: ModuleGraphSnapshot::new(graph),
        diagnostics: prior_diagnostics.append(&diagnostics),
    })
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
    let declarations = db.get(module_declarations_query(db, &node.path)?)?;
    for package in &declarations.semantic.package_roots {
        if graph.package_root(package).is_none()
            && let Some(path) = db.context().module_map.get_name(package)
        {
            graph.intern_package_root(package, path.clone());
        }
    }
    for path in ordered_used_module_paths(&declarations.semantic.used_module_paths) {
        add_semantic_used_module_path(db, graph, module_id, &path)?;
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
            | UsedModulePathProcessing::IfProvidesTraitImpl { .. }
            | UsedModulePathProcessing::IfProvidesImplicitTraitImpl { .. }
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
        activate_package_facade(db, graph, package)?;
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
    provider_demands: &std::collections::HashSet<nia_compiler_query::ProviderDemand>,
    diagnostics: &mut Vec<(SourcePath, Diagnostic)>,
) -> QueryResult<()> {
    let mut demands = provider_demands
        .iter()
        .filter(|demand| demand.source_path.identity() == node.path.identity())
        .cloned()
        .collect::<Vec<_>>();
    demands.sort_unstable_by_key(|demand| match &demand.request {
        nia_compiler_query::ProviderRequest::ModuleSemantic { .. }
        | nia_compiler_query::ProviderRequest::ModuleBody { .. } => 0,
        nia_compiler_query::ProviderRequest::Method {
            target_type_name: Some(_),
            ..
        }
        | nia_compiler_query::ProviderRequest::TraitImpl {
            target_type_name: Some(_),
            ..
        } => 1,
        nia_compiler_query::ProviderRequest::Method {
            target_type_name: None,
            ..
        }
        | nia_compiler_query::ProviderRequest::TraitImpl {
            target_type_name: None,
            ..
        } => 2,
    });
    for demand in demands {
        if let nia_compiler_query::ProviderRequest::ModuleSemantic { module_path } = demand.request
        {
            record_traversal_diagnostic(
                activate_semantic_provider_module(db, graph, &module_path),
                diagnostics,
                &module_path,
            )?;
            continue;
        }
        for import in imports {
            let processing = match demand.request {
                nia_compiler_query::ProviderRequest::Method {
                    target_type_name,
                    method_name,
                } => UsedModulePathProcessing::IfProvidesTraitMethod {
                    target_type_name,
                    associated_name: method_name,
                },
                nia_compiler_query::ProviderRequest::TraitImpl {
                    target_type_name,
                    trait_name,
                } => UsedModulePathProcessing::IfProvidesTraitImpl {
                    target_type_name,
                    trait_name,
                },
                nia_compiler_query::ProviderRequest::ModuleSemantic { .. } => continue,
                nia_compiler_query::ProviderRequest::ModuleBody { .. } => continue,
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

fn should_eager_add_declarations(context: &LoaderContext, node: &ModuleNode) -> bool {
    node.process_declared_children
        || (node.module_path.is_package_root()
            && context
                .package_roots_with_used_paths
                .contains(&node.module_path.package))
        || node.module_path.is_std_start_module()
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
        activate_package_facade(db, graph, package)?;
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
    package: SymbolId,
) -> TraversalResult<()> {
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
    for package in &declarations.semantic.package_roots {
        if graph.package_root(package).is_none()
            && let Some(path) = db.context().module_map.get_name(package)
        {
            graph.intern_package_root(package, path.clone());
        }
    }
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
        UsedModulePath::Package { package, .. } => graph.package_root(package),
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
    for package in &declarations.semantic.package_roots {
        if graph.package_root(package).is_none()
            && let Some(path) = db.context().module_map.get_name(package)
        {
            graph.intern_package_root(package, path.clone());
        }
    }
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
        UsedModulePathProcessing::IfProvidesTraitImpl { .. }
        | UsedModulePathProcessing::IfProvidesImplicitTraitImpl { .. }
        | UsedModulePathProcessing::IfProvidesTraitMethod { .. } => 3,
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
        process_provider_request(db, graph, current, &processing)?;
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
            let Some(reexport_source) =
                add_public_reexport_source_module(db, graph, current, segment, source_processing)?
            else {
                if processing.should_process_module() {
                    mark_process_used_paths_and_process(db, graph, current)?;
                }
                return Ok(Some(current));
            };
            if processing == UsedModulePathProcessing::Always && !is_terminal {
                mark_process_used_paths_and_process(db, graph, current)?;
            }
            process_reexport_provider_request(db, graph, reexport_facade, segment, &processing)?;
            current = reexport_source;
            if processing == UsedModulePathProcessing::IfSelectedItem && is_terminal {
                mark_process_used_paths_and_process(db, graph, current)?;
            }
            if is_terminal {
                process_provider_request(db, graph, current, &processing)?;
            }
            continue;
        };
        current = next;
        if processing.is_provider_demand() {
            process_provider_request(db, graph, current, &processing)?;
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
            process_provider_request(db, graph, current, &processing)?;
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
    diagnostics: &mut Vec<(SourcePath, Diagnostic)>,
) {
    match db.context().entry_runtime {
        EntryRuntime::None => {}
        EntryRuntime::Freestanding => {
            let std_root = graph.std_package_root().or_else(|| {
                db.context()
                    .module_map
                    .std_path()
                    .map(|path| graph.intern_std_package_root(path.clone()))
            });
            let Some(std_root) = std_root else { return };
            match graph.intern_declared_child(
                std_root,
                &known::START,
                nia_imports::Visibility::PublicPkg,
                Span::default(),
            ) {
                Ok(start_root) => graph.mark_executable_root_subtree(start_root),
                Err(diagnostic) => {
                    let path = graph
                        .get(std_root)
                        .map(|node| node.path.clone())
                        .or_else(|| db.context().module_map.std_path().cloned())
                        .unwrap_or_else(|| SourcePath::new("std"));
                    diagnostics.push((path, diagnostic));
                }
            }
        }
    }
}
