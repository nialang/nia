// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) struct LazyAssociatedValueResolver<'a> {
    type_store: &'a nia_ty::TypeStore,
    visible_extensions: &'a dyn Fn() -> QueryResult<Arc<VisibleExtensionsValue>>,
    cache: RefCell<Option<Arc<VisibleExtensionsValue>>>,
    failure: RefCell<Option<QueryError>>,
}

impl<'a> LazyAssociatedValueResolver<'a> {
    pub(super) fn new(
        type_store: &'a nia_ty::TypeStore,
        visible_extensions: &'a dyn Fn() -> QueryResult<Arc<VisibleExtensionsValue>>,
    ) -> Self {
        Self {
            type_store,
            visible_extensions,
            cache: RefCell::new(None),
            failure: RefCell::new(None),
        }
    }

    fn visible_extensions(&self) -> Option<Arc<VisibleExtensionsValue>> {
        if let Some(visible_extensions) = self.cache.borrow().as_ref() {
            return Some(visible_extensions.clone());
        }
        let visible_extensions = capture_query_failure(&self.failure, (self.visible_extensions)())?;
        *self.cache.borrow_mut() = Some(visible_extensions.clone());
        Some(visible_extensions)
    }

    pub(super) fn take_failure(&self) -> Option<QueryError> {
        self.failure.borrow_mut().take()
    }

    fn target_matches(
        type_store: &nia_ty::TypeStore,
        target_ty: InternedTyId,
        target: nia_value_resolve::AssociatedValueTarget,
    ) -> bool {
        match target {
            nia_value_resolve::AssociatedValueTarget::Primitive(primitive) => {
                matches!(type_store.get(target_ty), Some(TyKind::Primitive(found)) if *found == primitive)
            }
            nia_value_resolve::AssociatedValueTarget::Nominal(type_id) => {
                matches!(type_store.get(target_ty), Some(TyKind::Nominal { def_id, .. }) if *def_id == type_id)
            }
        }
    }
}

impl nia_value_resolve::AssociatedValueResolver for LazyAssociatedValueResolver<'_> {
    fn associated_value(
        &self,
        target: nia_value_resolve::AssociatedValueTarget,
        name: &SymbolId,
    ) -> Option<GlobalDefId> {
        let visible_extensions = self.visible_extensions()?;
        let mut matches = Vec::new();
        for extension_target in visible_extensions.methods.targets() {
            if !Self::target_matches(self.type_store, extension_target.target_ty, target) {
                continue;
            }
            for value in &extension_target.associated_values {
                if &value.name == name {
                    matches.push(value.def_id);
                }
            }
        }
        matches.sort();
        matches.dedup();
        let [def_id] = matches.as_slice() else {
            return None;
        };
        Some(*def_id)
    }
}

pub(super) fn provide_value_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ModuleValueResolution> {
    time_module_provider(db, "value_resolution", module_id, || {
        let active_item_tree = db.get(FullActiveModuleItemTreeQuery(module_id))?;
        let defs = full_module_defs_semantic(db, module_id)?;
        let graph = db.get(ModuleGraphQuery)?;
        let public_surfaces = db.get(PublicSurfacesQuery)?;
        let using_scope = db.get(ModuleUsingScopeQuery(module_id))?;
        let query_failure = RefCell::new(None);
        let program_defs = |module_id| {
            capture_query_failure(&query_failure, full_module_defs_semantic(db, module_id))
        };
        let visible_extensions = || db.get(VisibleExtensionsQuery(module_id));
        let associated_values =
            LazyAssociatedValueResolver::new(&db.context().type_store, &visible_extensions);
        let symbols = db.context().symbols();
        let mut resolution = nia_value_resolve::resolve_module_values_from_active_item_tree_with_associated_values_and_symbols_in_store(
            &active_item_tree,
            &defs,
            nia_value_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(graph.as_ref()),
            },
            &public_surfaces.surfaces,
            using_scope.as_ref(),
            nia_value_resolve::ValueResolveOptions::with_store(
                Some(&associated_values),
                Some(&symbols),
                db.context().node_store(),
            ),
        );
        if let Some(error) = query_failure
            .into_inner()
            .or_else(|| associated_values.take_failure())
        {
            Err(error)
        } else {
            let diagnostics = std::mem::take(&mut resolution.diagnostics);
            Ok(ModuleValueResolution {
                semantic: Arc::new(resolution),
                diagnostics: db.context().diagnostic_store.bundle(diagnostics),
            })
        }
    })
}

pub(super) fn provide_local_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ModuleLocalResolution> {
    let active_item_tree = db.get(FullActiveModuleItemTreeQuery(module_id))?;
    let defs = full_module_defs_semantic(db, module_id)?;
    let values = value_resolution_semantic(db, module_id)?;
    let symbols = db.context().symbols();
    let origins = nia_node_id::NodeOriginTable::with_store(db.context().node_store());
    let mut resolution =
        nia_local_resolve::resolve_module_locals_from_active_item_tree_with_origins_and_symbols(
            &active_item_tree,
            &defs,
            &values,
            None,
            &origins,
            &symbols,
        );
    let diagnostics = std::mem::take(&mut resolution.diagnostics);
    Ok(ModuleLocalResolution {
        semantic: Arc::new(resolution),
        diagnostics: db.context().diagnostic_store.bundle(diagnostics),
    })
}

pub(super) fn provide_semantic_use_table(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<nia_sema_ir::SemanticUseTable> {
    let values = value_resolution_semantic(db, module_id)?;
    let locals = local_resolution_semantic(db, module_id)?;
    let type_resolution = type_resolution_semantic(db, module_id)?;
    let type_lowering = type_lowering_semantic(db, module_id)?;
    let active_item_tree = db.get(FullActiveModuleItemTreeQuery(module_id))?;
    let needed_const_exprs = needed_const_exprs_for_active_item_tree(
        &db.context().type_store,
        &active_item_tree,
        &type_lowering,
    );
    let const_expr_value_resolution = if needed_const_exprs.is_empty() {
        None
    } else {
        let defs = full_module_defs_semantic(db, module_id)?;
        let public_surfaces = db.get(PublicSurfacesQuery)?;
        let using_scope = db.get(ModuleUsingScopeQuery(module_id))?;
        let graph = db.get(ModuleGraphQuery)?;
        let query_failure = RefCell::new(None);
        let visible_extensions = || db.get(VisibleExtensionsQuery(module_id));
        let associated_values =
            LazyAssociatedValueResolver::new(&db.context().type_store, &visible_extensions);
        let program_defs = |module_id| {
            capture_query_failure(&query_failure, full_module_defs_semantic(db, module_id))
        };
        let symbols = db.context().symbols();
        let resolution =
            nia_value_resolve::resolve_module_values_from_exprs_with_associated_values_and_symbols_in_store(
                type_lowering.const_exprs.iter().filter_map(|(id, expr)| {
                    needed_const_exprs.contains(id).then_some(expr.clone())
                }),
                &defs,
                nia_value_resolve::ProgramDefsContext {
                    defs: Some(&program_defs),
                    graph: Some(graph.as_ref()),
                },
                &public_surfaces.surfaces,
                using_scope.as_ref(),
                nia_value_resolve::ValueResolveOptions::with_store(
                    Some(&associated_values),
                    Some(&symbols),
                    db.context().node_store(),
                ),
            );
        if let Some(error) = query_failure
            .into_inner()
            .or_else(|| associated_values.take_failure())
        {
            return Err(error);
        }
        Some(resolution)
    };
    Ok(
        semantic_use_table_from_resolution_inputs_with_const_expr_values(SemanticUseInputs {
            module_id,
            node_store: db.context().node_store(),
            type_store: &db.context().type_store,
            active_item_tree: &active_item_tree,
            values: &values,
            const_expr_values: const_expr_value_resolution.as_ref(),
            const_expr_value_ids: Some(&needed_const_exprs),
            locals: &locals,
            type_resolution: &type_resolution,
            type_lowering: &type_lowering,
        }),
    )
}

pub(super) struct SemanticUseInputs<'a> {
    pub module_id: ModuleId,
    pub node_store: &'a nia_node_id::NodeStore,
    pub type_store: &'a nia_ty::TypeStore,
    pub active_item_tree: &'a ActiveModuleItemTree,
    pub values: &'a ValueResolution,
    pub const_expr_values: Option<&'a ValueResolution>,
    pub const_expr_value_ids: Option<&'a HashSet<GlobalConstExprId>>,
    pub locals: &'a LocalResolution,
    pub type_resolution: &'a TypeResolution,
    pub type_lowering: &'a TypeLowering,
}

pub(super) fn semantic_use_table_from_resolution_inputs_with_const_expr_values(
    input: SemanticUseInputs<'_>,
) -> nia_sema_ir::SemanticUseTable {
    let SemanticUseInputs {
        module_id,
        node_store,
        type_store,
        active_item_tree,
        values,
        const_expr_values: const_expr_value_resolution,
        const_expr_value_ids: const_expr_value_resolution_ids,
        locals,
        type_resolution,
        type_lowering,
    } = input;
    let mut builder = nia_sema_ir::SemanticUseTable::builder_with_node_store(node_store);

    for (key, local_use) in &locals.node_uses {
        match local_use {
            nia_local_resolve::LocalUse::Local(local_id) => {
                builder.insert_node_local_value_use(key.clone(), *local_id);
            }
            nia_local_resolve::LocalUse::Static(global_id) => {
                builder.insert_node_global_value_use(key.clone(), *global_id);
            }
            nia_local_resolve::LocalUse::TypePrefix => {
                let def_id = match type_resolution.node_type_names.get(key.site()) {
                    Some(nia_type_resolve::TypeNameResolution::Def(def_id)) => Some(GlobalDefId {
                        module_id,
                        def_id: *def_id,
                    }),
                    Some(nia_type_resolve::TypeNameResolution::External(def_id)) => Some(*def_id),
                    _ => None,
                };
                if let Some(def_id) = def_id {
                    builder.insert_node_type_prefix(key.clone(), def_id);
                }
            }
            nia_local_resolve::LocalUse::ModuleValue
            | nia_local_resolve::LocalUse::Module
            | nia_local_resolve::LocalUse::Unresolved => {}
        }
    }
    builder.extend_node_global_value_uses(
        values
            .node_qualified_values
            .iter()
            .map(|(key, global_id)| (key.clone(), *global_id)),
    );
    builder.extend_node_type_prefixes(
        values
            .node_qualified_type_prefixes
            .iter()
            .map(|(key, def_id)| (key.clone(), *def_id)),
    );
    builder.extend_node_builtin_associated_values(
        values
            .node_builtin_associated_values
            .iter()
            .map(|(key, value)| (key.clone(), *value)),
    );
    builder.extend_node_associated_const_projections(
        associated_const_projections_from_active_item_tree(
            type_store,
            active_item_tree,
            type_lowering,
        ),
    );
    builder.extend_node_associated_const_projections(
        associated_const_projections_from_const_exprs(
            type_store,
            &type_lowering.const_exprs,
            None,
            type_lowering,
        ),
    );
    builder.extend_node_const_generic_uses(
        type_resolution
            .node_const_generic_names
            .iter()
            .map(|(key, name)| (key.clone(), *name)),
    );
    if let Some(const_expr_value_resolution) = const_expr_value_resolution {
        let const_expr_nodes =
            const_expr_node_keys(&type_lowering.const_exprs, const_expr_value_resolution_ids);
        builder.extend_node_global_value_uses(
            const_expr_value_resolution
                .node_qualified_values
                .iter()
                .filter(|(key, _)| const_expr_nodes.contains(key))
                .map(|(key, global_id)| (key.clone(), *global_id)),
        );
        builder.extend_node_builtin_associated_values(
            const_expr_value_resolution
                .node_builtin_associated_values
                .iter()
                .filter(|(key, _)| const_expr_nodes.contains(key))
                .map(|(key, value)| (key.clone(), *value)),
        );
        builder.extend_node_associated_const_projections(
            associated_const_projections_from_const_exprs(
                type_store,
                &type_lowering.const_exprs,
                const_expr_value_resolution_ids,
                type_lowering,
            ),
        );
        for (key, resolution) in &const_expr_value_resolution.node_names {
            if !const_expr_nodes.contains(&key) {
                continue;
            }
            match resolution {
                nia_value_resolve::ValueNameResolution::Def(def_id) => {
                    builder.insert_node_global_value_use(
                        key.clone(),
                        GlobalDefId {
                            module_id,
                            def_id: *def_id,
                        },
                    );
                }
                nia_value_resolve::ValueNameResolution::External(global_id) => {
                    builder.insert_node_global_value_use(key.clone(), *global_id);
                }
                nia_value_resolve::ValueNameResolution::Module
                | nia_value_resolve::ValueNameResolution::LocalDeferred
                | nia_value_resolve::ValueNameResolution::Error => {}
            }
        }
    }
    for (key, resolution) in &values.node_names {
        match resolution {
            nia_value_resolve::ValueNameResolution::Def(def_id) => {
                builder.insert_node_global_value_use(
                    key.clone(),
                    GlobalDefId {
                        module_id,
                        def_id: *def_id,
                    },
                );
            }
            nia_value_resolve::ValueNameResolution::External(global_id) => {
                builder.insert_node_global_value_use(key.clone(), *global_id);
            }
            nia_value_resolve::ValueNameResolution::Module
            | nia_value_resolve::ValueNameResolution::LocalDeferred
            | nia_value_resolve::ValueNameResolution::Error => {}
        }
    }
    builder.extend_node_local_defs(
        locals
            .node_local_defs
            .iter()
            .map(|(key, local_id)| (key.clone(), *local_id)),
    );
    builder.extend_node_type_uses(
        type_lowering.versioned_type_uses_from_active_item_tree(active_item_tree),
    );
    builder.finish()
}

fn associated_const_projections_from_active_item_tree(
    type_store: &nia_ty::TypeStore,
    active_item_tree: &ActiveModuleItemTree,
    type_lowering: &TypeLowering,
) -> Vec<(
    nia_node_id::VersionedNodeKey,
    nia_sema_ir::AssociatedConstProjection,
)> {
    let mut collector = AssociatedConstProjectionCollector {
        type_store,
        type_lowering,
        projections: Vec::new(),
    };
    for item in &active_item_tree.items {
        collector.visit_item_tree_node(item);
    }
    collector.projections
}

fn associated_const_projections_from_const_exprs(
    type_store: &nia_ty::TypeStore,
    const_exprs: &HashMap<GlobalConstExprId, nia_ast::Expr>,
    ids: Option<&HashSet<GlobalConstExprId>>,
    type_lowering: &TypeLowering,
) -> Vec<(
    nia_node_id::VersionedNodeKey,
    nia_sema_ir::AssociatedConstProjection,
)> {
    let mut collector = AssociatedConstProjectionCollector {
        type_store,
        type_lowering,
        projections: Vec::new(),
    };
    for (id, expr) in const_exprs {
        if ids.is_some_and(|ids| !ids.contains(id)) {
            continue;
        }
        nia_ast_walk::Visitor::visit_expr(&mut collector, expr);
    }
    collector.projections
}

struct AssociatedConstProjectionCollector<'a> {
    type_store: &'a nia_ty::TypeStore,
    type_lowering: &'a TypeLowering,
    projections: Vec<(
        nia_node_id::VersionedNodeKey,
        nia_sema_ir::AssociatedConstProjection,
    )>,
}

impl AssociatedConstProjectionCollector<'_> {
    fn visit_item_tree_node(&mut self, item: &nia_item_tree::ItemTreeNode) {
        match &item.kind {
            nia_item_tree::ItemTreeNodeKind::Function(function) => {
                if let Some(body) = &function.body {
                    nia_ast_walk::Visitor::visit_block(self, body);
                }
            }
            nia_item_tree::ItemTreeNodeKind::Binding(binding) => {
                if let Some(value) = &binding.value {
                    nia_ast_walk::Visitor::visit_expr(self, value);
                }
            }
            nia_item_tree::ItemTreeNodeKind::Trait(item_trait) => {
                for method in &item_trait.methods {
                    if let Some(body) = &method.function.body {
                        nia_ast_walk::Visitor::visit_block(self, body);
                    }
                }
            }
            nia_item_tree::ItemTreeNodeKind::Extend(extend) => {
                for associated_value in &extend.associated_values {
                    if let Some(value) = &associated_value.binding.value {
                        nia_ast_walk::Visitor::visit_expr(self, value);
                    }
                }
                for method in &extend.methods {
                    if let Some(body) = &method.function.body {
                        nia_ast_walk::Visitor::visit_block(self, body);
                    }
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

    fn record_projection(
        &mut self,
        expr: &nia_ast::Expr,
        target: &nia_ast::TypeRef,
        trait_ref: &nia_ast::TypeRef,
        name: &SymbolId,
    ) {
        let Some(self_ty) = self.type_lowering.ty_for_key(&target.node_key) else {
            return;
        };
        let Some(trait_ty) = self.type_lowering.ty_for_key(&trait_ref.node_key) else {
            return;
        };
        let Some((trait_id, trait_args, trait_const_args)) =
            self.trait_id_and_args_from_ty(trait_ty)
        else {
            return;
        };
        self.projections.push((
            expr.node_key.clone(),
            nia_sema_ir::AssociatedConstProjection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name: *name,
            },
        ));
    }

    fn trait_id_and_args_from_ty(
        &self,
        ty: InternedTyId,
    ) -> Option<(
        nia_ty::TraitId,
        Vec<InternedTyId>,
        Vec<nia_ty::ConstGenericArg>,
    )> {
        match self.type_store.get(ty)? {
            nia_ty::TyKind::Nominal {
                def_id,
                args,
                const_args,
            } => Some((
                nia_ty::TraitId::Source(*def_id),
                args.clone(),
                const_args.clone(),
            )),
            nia_ty::TyKind::BuiltinTrait { trait_id, args } => Some((
                nia_ty::TraitId::Builtin(*trait_id),
                args.clone(),
                Vec::new(),
            )),
            nia_ty::TyKind::TraitObject {
                trait_id,
                trait_args,
                trait_const_args,
                ..
            }
            | nia_ty::TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                ..
            } => Some((*trait_id, trait_args.clone(), trait_const_args.clone())),
            _ => None,
        }
    }
}

impl<'ast> nia_ast_walk::Visitor<'ast> for AssociatedConstProjectionCollector<'_> {
    fn visit_expr(&mut self, expr: &'ast nia_ast::Expr) {
        if let nia_ast::ExprKind::Qualified { lhs, name } = &expr.kind
            && let nia_ast::ExprKind::TraitTarget { ty, trait_ref } = &lhs.kind
        {
            self.record_projection(expr, ty, trait_ref, name);
        }
        nia_ast_walk::walk_expr(self, expr);
    }
}

fn const_expr_node_keys(
    const_exprs: &HashMap<GlobalConstExprId, nia_ast::Expr>,
    ids: Option<&HashSet<GlobalConstExprId>>,
) -> HashSet<nia_node_id::VersionedNodeKey> {
    struct ExprNodeCollector {
        keys: HashSet<nia_node_id::VersionedNodeKey>,
    }

    impl<'ast> nia_ast_walk::Visitor<'ast> for ExprNodeCollector {
        fn visit_expr(&mut self, expr: &'ast nia_ast::Expr) {
            self.keys.insert(expr.node_key.clone());
            nia_ast_walk::walk_expr(self, expr);
        }
    }

    let mut collector = ExprNodeCollector {
        keys: HashSet::new(),
    };
    for (id, expr) in const_exprs {
        if ids.is_some_and(|ids| !ids.contains(id)) {
            continue;
        }
        nia_ast_walk::Visitor::visit_expr(&mut collector, expr);
    }
    collector.keys
}

pub(super) fn needed_const_exprs_for_active_item_tree(
    type_store: &nia_ty::TypeStore,
    active_item_tree: &ActiveModuleItemTree,
    type_lowering: &TypeLowering,
) -> HashSet<GlobalConstExprId> {
    if type_lowering.const_exprs.is_empty() {
        return HashSet::new();
    }
    let candidate_ids = type_lowering
        .const_exprs
        .keys()
        .copied()
        .collect::<HashSet<_>>();
    let mut out = HashSet::new();
    let mut seen = HashSet::new();
    for (_, ty) in type_lowering.versioned_type_uses_from_active_item_tree(active_item_tree) {
        collect_array_len_const_exprs_in_ty(type_store, ty, &candidate_ids, &mut out, &mut seen);
        if out.len() == candidate_ids.len() {
            break;
        }
    }
    out
}

pub(super) fn const_expr_subset_for_ids(
    const_exprs: &HashMap<GlobalConstExprId, nia_ast::Expr>,
    ids: &HashSet<GlobalConstExprId>,
) -> HashMap<GlobalConstExprId, nia_ast::Expr> {
    const_exprs
        .iter()
        .filter_map(|(id, expr)| ids.contains(id).then_some((*id, expr.clone())))
        .collect()
}

fn collect_array_len_const_exprs_in_ty(
    type_store: &nia_ty::TypeStore,
    ty: InternedTyId,
    candidate_ids: &HashSet<GlobalConstExprId>,
    out: &mut HashSet<GlobalConstExprId>,
    seen: &mut HashSet<InternedTyId>,
) {
    if !seen.insert(ty) {
        return;
    }
    match type_store.get(ty) {
        Some(TyKind::Array { len, elem }) => {
            collect_array_len_const_exprs_in_len(type_store, len, candidate_ids, out, seen);
            collect_array_len_const_exprs_in_ty(type_store, *elem, candidate_ids, out, seen);
        }
        Some(
            TyKind::Optional { elem }
            | TyKind::Pointer { elem, .. }
            | TyKind::VolatilePointer { elem, .. }
            | TyKind::Slice { elem, .. }
            | TyKind::SlicePointee { elem },
        ) => {
            collect_array_len_const_exprs_in_ty(type_store, *elem, candidate_ids, out, seen);
        }
        Some(TyKind::ErrorUnion { error, value }) => {
            collect_array_len_const_exprs_in_ty(type_store, *error, candidate_ids, out, seen);
            collect_array_len_const_exprs_in_ty(type_store, *value, candidate_ids, out, seen);
        }
        Some(TyKind::Tuple(elems)) => {
            for elem in elems {
                collect_array_len_const_exprs_in_ty(type_store, *elem, candidate_ids, out, seen);
            }
        }
        Some(TyKind::Range {
            bound: Some(bound), ..
        }) => {
            collect_array_len_const_exprs_in_ty(type_store, *bound, candidate_ids, out, seen);
        }
        Some(TyKind::FunctionPointer {
            params,
            return_type,
            ..
        }) => {
            for param in params {
                collect_array_len_const_exprs_in_ty(type_store, *param, candidate_ids, out, seen);
            }
            collect_array_len_const_exprs_in_ty(type_store, *return_type, candidate_ids, out, seen);
        }
        Some(TyKind::Nominal {
            args, const_args, ..
        }) => {
            for arg in args {
                collect_array_len_const_exprs_in_ty(type_store, *arg, candidate_ids, out, seen);
            }
            for arg in const_args {
                collect_array_len_const_exprs_in_ty(type_store, arg.ty, candidate_ids, out, seen);
                collect_array_len_const_exprs_in_const_arg(arg, candidate_ids, out);
            }
        }
        Some(TyKind::BuiltinTrait { args, .. }) => {
            for arg in args {
                collect_array_len_const_exprs_in_ty(type_store, *arg, candidate_ids, out, seen);
            }
        }
        Some(
            TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            }
            | TyKind::TraitObjectPointee {
                trait_args,
                associated_type_bindings,
                ..
            },
        ) => {
            for arg in trait_args {
                collect_array_len_const_exprs_in_ty(type_store, *arg, candidate_ids, out, seen);
            }
            for binding in associated_type_bindings {
                collect_array_len_const_exprs_in_ty(
                    type_store,
                    binding.ty,
                    candidate_ids,
                    out,
                    seen,
                );
            }
        }
        Some(TyKind::Projection {
            self_ty,
            trait_args,
            ..
        }) => {
            collect_array_len_const_exprs_in_ty(type_store, *self_ty, candidate_ids, out, seen);
            for arg in trait_args {
                collect_array_len_const_exprs_in_ty(type_store, *arg, candidate_ids, out, seen);
            }
        }
        Some(
            TyKind::Range { bound: None, .. }
            | TyKind::Opaque
            | TyKind::Error
            | TyKind::ConstOnly
            | TyKind::SelfParam
            | TyKind::GenericParam(_)
            | TyKind::Primitive(_)
            | TyKind::BuiltinType(_)
            | TyKind::Vector { .. },
        )
        | None => {}
    }
}

fn collect_array_len_const_exprs_in_len(
    type_store: &nia_ty::TypeStore,
    len: &ArrayLenTy,
    candidate_ids: &HashSet<GlobalConstExprId>,
    out: &mut HashSet<GlobalConstExprId>,
    seen: &mut HashSet<InternedTyId>,
) {
    match len {
        ArrayLenTy::ConstExpr(id) => {
            if candidate_ids.contains(id) {
                out.insert(*id);
            }
        }
        ArrayLenTy::Builtin { ty, .. } => {
            collect_array_len_const_exprs_in_ty(type_store, *ty, candidate_ids, out, seen);
        }
        ArrayLenTy::Infer | ArrayLenTy::GenericParam(_) | ArrayLenTy::ConstValue(_) => {}
    }
}

fn collect_array_len_const_exprs_in_const_arg(
    arg: &nia_ty::ConstGenericArg,
    candidate_ids: &HashSet<GlobalConstExprId>,
    out: &mut HashSet<GlobalConstExprId>,
) {
    if let nia_ty::ConstGenericValue::ConstExpr(id) = arg.value
        && candidate_ids.contains(&id)
    {
        out.insert(id);
    }
}
