// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_ast::{BindingItem, FunctionItem};
use std::collections::VecDeque;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::query) struct ExecutableValueRefEdges {
    pub(in crate::query) functions: HashSet<GlobalDefId>,
    pub(in crate::query) globals: HashSet<GlobalDefId>,
}

#[derive(Clone, Default)]
pub(super) struct ExecutableValueRefIndex {
    pub(super) functions: HashMap<GlobalDefId, ExecutableValueRefEdges>,
    pub(super) globals: HashMap<GlobalDefId, ExecutableValueRefEdges>,
}

pub(in crate::query) fn provide_executable_value_ref_edges(
    db: &QueryDb<CompilerContext>,
    owner: GlobalDefId,
) -> QueryResult<ExecutableValueRefEdges> {
    time_module_provider(db, "executable_value_ref_edges", owner.module_id, || {
        let program_sources = db.get(FrontendProgramSourcesQuery)?;
        let cache_input = program_sources
            .as_ref()
            .as_ref()
            .and_then(|program_sources| {
                let source = program_sources.by_module.get(&owner.module_id)?;
                let namespace = db.context().frontend_cache_namespace();
                let key = crate::FrontendExecutableValueRefEdgesCacheKey::new(
                    namespace,
                    &source.module,
                    owner.def_id,
                    program_sources.fingerprint,
                );
                Some((program_sources, source, namespace, key))
            });
        let cached = if let Some(cache) = db.context().signature_cache.as_ref()
            && let Some((program_sources, source, namespace, key)) = cache_input
        {
            match cache.load_executable_value_ref_edges(
                crate::signature_cache::ExecutableValueRefEdgesIdentity {
                    key,
                    namespace,
                    module: &source.module,
                    owner: owner.def_id,
                    program_sources: program_sources.fingerprint,
                },
                &program_sources.module_by_path,
            ) {
                Ok(lookup) => {
                    match lookup {
                        crate::signature_cache::ExecutableValueRefEdgesLookup::Hit(_) => {
                            nia_timing::emit_counter(
                                "frontend.executable_value_ref_edges_reuse_hits",
                                1,
                            );
                        }
                        crate::signature_cache::ExecutableValueRefEdgesLookup::NotFound => {
                            nia_timing::emit_counter(
                                "frontend.executable_value_ref_edges_reuse_miss_not_found",
                                1,
                            );
                        }
                        crate::signature_cache::ExecutableValueRefEdgesLookup::Corrupt => {
                            nia_timing::emit_counter(
                                "frontend.executable_value_ref_edges_reuse_miss_corrupt",
                                1,
                            );
                        }
                    }
                    Some(lookup)
                }
                Err(_) => {
                    nia_timing::emit_counter(
                        "frontend.executable_value_ref_edges_reuse_miss_read_error",
                        1,
                    );
                    None
                }
            }
        } else {
            None
        };
        // Verification deliberately recomputes valid hits. A mismatch below
        // replaces the persisted entry, turning verification into cache repair.
        let cached = if db.context().verify_frontend_cache {
            cached
        } else {
            match cached {
                Some(crate::signature_cache::ExecutableValueRefEdgesLookup::Hit(cached)) => {
                    return Ok(ExecutableValueRefEdges {
                        functions: cached.functions,
                        globals: cached.globals,
                    });
                }
                cached => cached,
            }
        };

        let edges = if let Some(item_input) = db.get(ExecutableValueRefItemQuery(owner))?.as_ref() {
            let full_active_item_tree = db.get(FullActiveModuleItemTreeQuery(owner.module_id))?;
            let active_item_tree =
                executable_value_ref_active_item_tree(item_input, &full_active_item_tree);
            let defs = module_defs_semantic(db, owner.module_id)?;
            let query_failure = RefCell::new(None);
            let program_defs = |module_id| {
                capture_query_failure(&query_failure, module_defs_semantic(db, module_id))
            };
            let graph = QueryModuleGraphLookup::new(db)?;
            let public_surfaces = QueryPublicSurfaceLookup::new(db);
            let using_scope = QueryUsingScopeLookup::new(db, owner.module_id);
            let visible_extensions = || db.get(VisibleExtensionsQuery(owner.module_id));
            let associated_values =
                LazyAssociatedValueResolver::new(&db.context().type_store, &visible_extensions);
            let symbols = db.context().symbols();
            let values = nia_value_resolve::resolve_module_values_from_active_item_tree_with_associated_values_and_symbols_in_store(
                &active_item_tree,
                &defs,
                nia_value_resolve::ProgramDefsContext {
                    defs: Some(&program_defs),
                    graph: Some(&graph),
                },
                &public_surfaces,
                &using_scope,
                nia_value_resolve::ValueResolveOptions::with_store(
                    Some(&associated_values),
                    Some(&symbols),
                    db.context().node_store(),
                ),
            );
            if let Some(error) = query_failure
                .into_inner()
                .or_else(|| graph.take_failure())
                .or_else(|| public_surfaces.take_failure())
                .or_else(|| using_scope.take_failure())
                .or_else(|| associated_values.take_failure())
            {
                return Err(error);
            }
            let origins = nia_node_id::NodeOriginTable::with_store(db.context().node_store());
            let locals =
                nia_local_resolve::resolve_module_locals_from_filtered_active_item_tree_with_origins_and_symbols(
                    &active_item_tree,
                    &full_active_item_tree,
                    &defs,
                    &values,
                    None,
                    &origins,
                    &symbols,
                );
            let mut index = ExecutableValueRefIndex::default();
            collect_executable_value_ref_index_for_items(
                db,
                owner.module_id,
                &active_item_tree.items,
                &defs,
                &values,
                &locals,
                &mut index,
            )?;
            index
                .functions
                .remove(&owner)
                .or_else(|| index.globals.remove(&owner))
                .unwrap_or_default()
        } else {
            ExecutableValueRefEdges::default()
        };

        if let Some(cache) = &db.context().signature_cache
            && let Some((program_sources, source, namespace, key)) = cache_input
        {
            let stable_edges = crate::signature_cache::CachedExecutableValueRefEdges {
                functions: edges.functions.clone(),
                globals: edges.globals.clone(),
            };
            let replace = matches!(
                &cached,
                Some(crate::signature_cache::ExecutableValueRefEdgesLookup::Hit(cached))
                    if cached != &stable_edges
            );
            if replace {
                cache.remove_executable_value_ref_edges(key);
            }
            let published = cache.publish_executable_value_ref_edges(
                crate::signature_cache::ExecutableValueRefEdgesIdentity {
                    key,
                    namespace,
                    module: &source.module,
                    owner: owner.def_id,
                    program_sources: program_sources.fingerprint,
                },
                &stable_edges,
                &program_sources.path_by_module,
                replace,
            );
            nia_timing::emit_counter(
                if published.is_ok() {
                    "frontend.executable_value_ref_edges_cacheable"
                } else {
                    "frontend.executable_value_ref_edges_uncacheable"
                },
                1,
            );
        }
        Ok(edges)
    })
}

pub(super) fn walk_executable_value_ref_closure(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    functions: &mut HashSet<GlobalDefId>,
    globals: &HashSet<GlobalDefId>,
    checked_functions: Option<&HashSet<GlobalDefId>>,
    mut on_function: impl FnMut(GlobalDefId) -> bool,
    mut on_global: impl FnMut(GlobalDefId) -> bool,
) -> QueryResult<bool> {
    let mut changed = false;
    let mut pending_functions = functions.iter().copied().collect::<VecDeque<_>>();
    let mut scanned_functions = HashSet::with_capacity(functions.len());

    // Globals seed the graph, while only local, not-yet-checked functions are
    // recursively scanned. Cross-module edges are reported to the caller and
    // expanded by that module's own reachability pass.
    for global in globals {
        let edges = db.get(ExecutableValueRefEdgesQuery(*global))?;
        changed |= visit_executable_value_ref_edges(
            module_id,
            functions,
            &mut pending_functions,
            checked_functions,
            &edges,
            &mut on_function,
            &mut on_global,
        );
    }
    while let Some(function) = pending_functions.pop_front() {
        if !scanned_functions.insert(function) {
            continue;
        }
        let edges = db.get(ExecutableValueRefEdgesQuery(function))?;
        changed |= visit_executable_value_ref_edges(
            module_id,
            functions,
            &mut pending_functions,
            checked_functions,
            &edges,
            &mut on_function,
            &mut on_global,
        );
    }
    Ok(changed)
}

fn visit_executable_value_ref_edges(
    module_id: ModuleId,
    functions: &mut HashSet<GlobalDefId>,
    pending_functions: &mut VecDeque<GlobalDefId>,
    checked_functions: Option<&HashSet<GlobalDefId>>,
    edges: &ExecutableValueRefEdges,
    on_function: &mut impl FnMut(GlobalDefId) -> bool,
    on_global: &mut impl FnMut(GlobalDefId) -> bool,
) -> bool {
    let mut changed = false;
    for global_id in &edges.functions {
        changed |= on_function(*global_id);
        if global_id.module_id == module_id
            && checked_functions.is_none_or(|checked| !checked.contains(global_id))
            && functions.insert(*global_id)
        {
            pending_functions.push_back(*global_id);
            changed = true;
        }
    }
    for global_id in &edges.globals {
        changed |= on_global(*global_id);
    }
    changed
}

impl ExecutableValueRefEdges {
    fn insert_edge(
        &mut self,
        db: &QueryDb<CompilerContext>,
        global_id: GlobalDefId,
    ) -> QueryResult<bool> {
        let defs = full_module_defs_semantic(db, global_id.module_id)?;
        let Some(def) = defs.defs.get(global_id.def_id) else {
            return Ok(false);
        };
        Ok(match def.kind {
            DefKind::Function | DefKind::Method | DefKind::TraitMethod => {
                let signatures = db.get(SignatureItemSignaturesQuery(
                    global_id.module_id,
                    nia_item_tree::SignatureItemSet::Functions,
                ))?;
                let Some(signature) = signatures.semantic.functions.get(&global_id.def_id) else {
                    return Ok(false);
                };
                if !signature.has_body {
                    return Ok(false);
                }
                self.functions.insert(global_id)
            }
            DefKind::Global => self.globals.insert(global_id),
            DefKind::Const
            | DefKind::Struct
            | DefKind::StructField
            | DefKind::Union
            | DefKind::UnionField
            | DefKind::Enum
            | DefKind::EnumVariant
            | DefKind::EnumVariantField
            | DefKind::TypeAlias
            | DefKind::Trait
            | DefKind::TraitAssociatedType
            | DefKind::Module => false,
        })
    }
}

pub(super) fn executable_value_ref_active_item_tree(
    input: &ExecutableValueRefItemInput,
    full_active_item_tree: &ActiveModuleItemTree,
) -> ActiveModuleItemTree {
    let mut item = full_active_item_tree.items[input.item_index].clone();
    match &mut item.kind {
        nia_item_tree::ItemTreeNodeKind::Trait(item_trait) => {
            item_trait
                .methods
                .retain(|method| method.function.node_key == input.owner_node_key);
        }
        nia_item_tree::ItemTreeNodeKind::Extend(extend) => {
            extend
                .methods
                .retain(|method| method.function.node_key == input.owner_node_key);
            extend
                .associated_values
                .retain(|value| value.binding.node_key == input.owner_node_key);
        }
        _ => {}
    }
    ActiveModuleItemTree::new(vec![item], full_active_item_tree.inactive_spans.clone())
}

pub(super) fn collect_executable_value_ref_index_for_items(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    items: &[nia_item_tree::ItemTreeNode],
    defs: &DefCollection,
    values: &ValueResolution,
    locals: &LocalResolution,
    index: &mut ExecutableValueRefIndex,
) -> QueryResult<()> {
    for item in items {
        match &item.kind {
            nia_item_tree::ItemTreeNodeKind::Function(function) => {
                collect_executable_value_ref_index_for_function(
                    db, module_id, defs, values, locals, function, index,
                )?;
            }
            nia_item_tree::ItemTreeNodeKind::Binding(binding) => {
                collect_executable_value_ref_index_for_binding(
                    db, module_id, defs, values, locals, binding, index,
                )?;
            }
            nia_item_tree::ItemTreeNodeKind::Trait(item_trait) => {
                for method in &item_trait.methods {
                    collect_executable_value_ref_index_for_function(
                        db,
                        module_id,
                        defs,
                        values,
                        locals,
                        &method.function,
                        index,
                    )?;
                }
            }
            nia_item_tree::ItemTreeNodeKind::Extend(extend) => {
                for associated_value in &extend.associated_values {
                    collect_executable_value_ref_index_for_binding(
                        db,
                        module_id,
                        defs,
                        values,
                        locals,
                        &associated_value.binding,
                        index,
                    )?;
                }
                for method in &extend.methods {
                    collect_executable_value_ref_index_for_function(
                        db,
                        module_id,
                        defs,
                        values,
                        locals,
                        &method.function,
                        index,
                    )?;
                }
            }
            nia_item_tree::ItemTreeNodeKind::Module(_)
            | nia_item_tree::ItemTreeNodeKind::Using(_)
            | nia_item_tree::ItemTreeNodeKind::Struct(_)
            | nia_item_tree::ItemTreeNodeKind::Union(_)
            | nia_item_tree::ItemTreeNodeKind::Enum(_)
            | nia_item_tree::ItemTreeNodeKind::TypeAlias(_) => {}
        }
    }
    Ok(())
}

fn collect_executable_value_ref_index_for_function(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    defs: &DefCollection,
    values: &ValueResolution,
    locals: &LocalResolution,
    function: &FunctionItem,
    index: &mut ExecutableValueRefIndex,
) -> QueryResult<()> {
    if function.body.is_none() {
        return Ok(());
    }
    let Some(def_id) = defs.def_nodes.get(&function.node_key) else {
        return Ok(());
    };
    let owner = GlobalDefId { module_id, def_id };
    let edges = index.functions.entry(owner).or_default();
    collect_executable_value_ref_edges_from_function(db, module_id, function, values, locals, edges)
}

fn collect_executable_value_ref_index_for_binding(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    defs: &DefCollection,
    values: &ValueResolution,
    locals: &LocalResolution,
    binding: &BindingItem,
    index: &mut ExecutableValueRefIndex,
) -> QueryResult<()> {
    if binding.is_const() || binding.value.is_none() {
        return Ok(());
    }
    let Some(def_id) = defs.def_nodes.get(&binding.node_key) else {
        return Ok(());
    };
    let owner = GlobalDefId { module_id, def_id };
    let edges = index.globals.entry(owner).or_default();
    collect_executable_value_ref_edges_from_binding(db, module_id, binding, values, locals, edges)
}

fn collect_executable_value_ref_edges_from_function(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    function: &FunctionItem,
    values: &ValueResolution,
    locals: &LocalResolution,
    edges: &mut ExecutableValueRefEdges,
) -> QueryResult<()> {
    let mut collector = ExecutableValueRefCollector::new(db, module_id, values, locals, edges);
    nia_ast_walk::Visitor::visit_function(&mut collector, function);
    collector.finish()
}

fn collect_executable_value_ref_edges_from_binding(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    binding: &BindingItem,
    values: &ValueResolution,
    locals: &LocalResolution,
    edges: &mut ExecutableValueRefEdges,
) -> QueryResult<()> {
    let mut collector = ExecutableValueRefCollector::new(db, module_id, values, locals, edges);
    if let Some(ty) = &binding.ty {
        nia_ast_walk::Visitor::visit_type(&mut collector, ty);
    }
    if let Some(value) = &binding.value {
        nia_ast_walk::Visitor::visit_expr(&mut collector, value);
    }
    collector.finish()
}

struct ExecutableValueRefCollector<'a> {
    db: &'a QueryDb<CompilerContext>,
    module_id: ModuleId,
    values: &'a ValueResolution,
    locals: &'a LocalResolution,
    edges: &'a mut ExecutableValueRefEdges,
    failure: Option<QueryError>,
}

impl<'a> ExecutableValueRefCollector<'a> {
    fn new(
        db: &'a QueryDb<CompilerContext>,
        module_id: ModuleId,
        values: &'a ValueResolution,
        locals: &'a LocalResolution,
        edges: &'a mut ExecutableValueRefEdges,
    ) -> Self {
        Self {
            db,
            module_id,
            values,
            locals,
            edges,
            failure: None,
        }
    }

    fn collect_key(&mut self, key: &nia_node_id::VersionedNodeKey) {
        if self.failure.is_some() {
            return;
        }
        if let Err(error) = collect_executable_value_ref_edge_for_key(
            self.db,
            self.module_id,
            self.values,
            self.locals,
            self.edges,
            key,
        ) {
            self.failure = Some(error);
        }
    }

    fn finish(self) -> QueryResult<()> {
        match self.failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl<'ast> nia_ast_walk::Visitor<'ast> for ExecutableValueRefCollector<'_> {
    fn visit_expr(&mut self, expr: &'ast nia_ast::Expr) {
        self.collect_key(&expr.node_key);
        match &expr.kind {
            nia_ast::ExprKind::ArrayLiteral {
                elems: nia_ast::ArrayElements::Repeat { value, .. },
            } => self.visit_expr(value),
            _ => nia_ast_walk::walk_expr(self, expr),
        }
    }

    fn visit_type(&mut self, _ty: &'ast nia_ast::TypeRef) {
        // Type-level expressions are owned by const/layout reachability, not
        // runtime value reachability. Repeat counts follow the same rule, so
        // visit_expr handles only the repeated runtime value above.
    }
}

fn collect_executable_value_ref_edge_for_key(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    values: &ValueResolution,
    locals: &LocalResolution,
    edges: &mut ExecutableValueRefEdges,
    key: &nia_node_id::VersionedNodeKey,
) -> QueryResult<()> {
    match locals.node_uses.get(key) {
        Some(nia_local_resolve::LocalUse::Static(global_id)) => {
            edges.insert_edge(db, *global_id)?;
            return Ok(());
        }
        Some(nia_local_resolve::LocalUse::Local(_)) => return Ok(()),
        Some(nia_local_resolve::LocalUse::ModuleValue)
        | Some(nia_local_resolve::LocalUse::Module)
        | Some(nia_local_resolve::LocalUse::TypePrefix)
        | Some(nia_local_resolve::LocalUse::Unresolved)
        | None => {}
    }
    if let Some(global_id) = values
        .node_names
        .get(key)
        .and_then(|resolution| match resolution {
            nia_value_resolve::ValueNameResolution::Def(def_id) => Some(GlobalDefId {
                module_id,
                def_id: *def_id,
            }),
            nia_value_resolve::ValueNameResolution::External(global_id) => Some(*global_id),
            nia_value_resolve::ValueNameResolution::Module
            | nia_value_resolve::ValueNameResolution::LocalDeferred
            | nia_value_resolve::ValueNameResolution::Error => None,
        })
    {
        edges.insert_edge(db, global_id)?;
    }
    if let Some(global_id) = values.node_qualified_values.get(key).copied() {
        edges.insert_edge(db, global_id)?;
    }
    Ok(())
}
