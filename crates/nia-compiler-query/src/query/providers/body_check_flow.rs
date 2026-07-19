// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn active_item_tree_for_body_check_filter(
    module_id: ModuleId,
    defs: &DefCollection,
    active_item_tree: &ActiveModuleItemTree,
    filter: nia_body_check::BodyCheckFilter<'_>,
) -> ActiveModuleItemTree {
    ActiveModuleItemTree::new(
        active_item_tree
            .items
            .iter()
            .cloned()
            .map(|mut item| {
                filter_item_tree_node_for_body_check(module_id, defs, &mut item, filter);
                item
            })
            .collect(),
        active_item_tree.inactive_spans.clone(),
    )
}

fn filter_item_tree_node_for_body_check(
    module_id: ModuleId,
    defs: &DefCollection,
    item: &mut nia_item_tree::ItemTreeNode,
    filter: nia_body_check::BodyCheckFilter<'_>,
) {
    match &mut item.kind {
        nia_item_tree::ItemTreeNodeKind::Function(function) => {
            if !function.is_const
                && !body_check_filter_includes_function(module_id, defs, &function.node_key, filter)
            {
                function.body = None;
            }
        }
        nia_item_tree::ItemTreeNodeKind::Binding(binding) => {
            if !binding.is_const()
                && !body_check_filter_includes_global(module_id, defs, &binding.node_key, filter)
            {
                binding.value = None;
            }
        }
        nia_item_tree::ItemTreeNodeKind::Trait(item_trait) => {
            for method in &mut item_trait.methods {
                if !method.function.is_const
                    && !body_check_filter_includes_function(
                        module_id,
                        defs,
                        &method.function.node_key,
                        filter,
                    )
                {
                    method.function.body = None;
                }
            }
        }
        nia_item_tree::ItemTreeNodeKind::Extend(extend) => {
            for method in &mut extend.methods {
                if !method.function.is_const
                    && !body_check_filter_includes_function(
                        module_id,
                        defs,
                        &method.function.node_key,
                        filter,
                    )
                {
                    method.function.body = None;
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

fn body_check_filter_includes_function(
    module_id: ModuleId,
    defs: &DefCollection,
    node_key: &nia_node_id::VersionedNodeKey,
    filter: nia_body_check::BodyCheckFilter<'_>,
) -> bool {
    body_check_filter_includes_def(module_id, defs, node_key, filter, true)
}

fn body_check_filter_includes_global(
    module_id: ModuleId,
    defs: &DefCollection,
    node_key: &nia_node_id::VersionedNodeKey,
    filter: nia_body_check::BodyCheckFilter<'_>,
) -> bool {
    body_check_filter_includes_def(module_id, defs, node_key, filter, false)
}

fn body_check_filter_includes_def(
    module_id: ModuleId,
    defs: &DefCollection,
    node_key: &nia_node_id::VersionedNodeKey,
    filter: nia_body_check::BodyCheckFilter<'_>,
    is_function: bool,
) -> bool {
    let Some(def_id) = defs.def_nodes.get(node_key) else {
        return true;
    };
    let global_def_id = GlobalDefId { module_id, def_id };
    match filter {
        nia_body_check::BodyCheckFilter::All => true,
        nia_body_check::BodyCheckFilter::ReachableFunctions(functions) => {
            !is_function || functions.contains(&global_def_id)
        }
        nia_body_check::BodyCheckFilter::ReachableItems {
            functions,
            globals,
            already_checked_functions,
            already_checked_globals,
        } => {
            if is_function {
                functions.contains(&global_def_id)
                    && already_checked_functions
                        .is_none_or(|checked| !checked.contains(&global_def_id))
            } else {
                globals.contains(&global_def_id)
                    && already_checked_globals
                        .is_none_or(|checked| !checked.contains(&global_def_id))
            }
        }
    }
}

pub(super) fn provide_body_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_body_check::BodyCheck {
    time_module_provider(db, "body_check", module_id, || {
        body_check_with_filter(db, module_id, nia_body_check::BodyCheckFilter::All)
    })
}

fn body_check_with_filter(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    filter: nia_body_check::BodyCheckFilter<'_>,
) -> nia_body_check::BodyCheck {
    body_check_with_filter_and_layouts(db, module_id, filter, None, None, None)
}

pub(super) fn body_check_resolution_inputs_for_filter(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    filter: nia_body_check::BodyCheckFilter<'_>,
    context: BodyCheckResolutionContext<'_>,
) -> BodyCheckResolutionInputs {
    match filter {
        nia_body_check::BodyCheckFilter::All => BodyCheckResolutionInputs {
            active_item_tree: context.active_item_tree,
            values: db.get(ValueResolutionQuery(module_id)),
            locals: db.get(LocalResolutionQuery(module_id)),
            semantic_uses: db.get(SemanticUseTableQuery(module_id)),
        },
        _ => {
            let filtered_active_item_tree = Arc::new(time_module_provider(
                db,
                "executable_body_check.filter_item_tree",
                module_id,
                || {
                    active_item_tree_for_body_check_filter(
                        module_id,
                        context.defs,
                        &context.active_item_tree,
                        filter,
                    )
                },
            ));
            let program_defs = |module_id| Some(db.get(FullModuleDefsQuery(module_id)));
            let public_surfaces = time_module_provider(
                db,
                "executable_body_check.public_surfaces",
                module_id,
                || db.get(PublicSurfacesQuery),
            );
            let using_scope = time_module_provider(
                db,
                "executable_body_check.module_using_scope",
                module_id,
                || db.get(ModuleUsingScopeQuery(module_id)),
            );
            let visible_extensions = || {
                time_module_provider(
                    db,
                    "executable_body_check.visible_extensions",
                    module_id,
                    || db.get(VisibleExtensionsQuery(module_id)),
                )
            };
            let associated_values =
                LazyAssociatedValueResolver::new(&db.context().type_store, &visible_extensions);
            let symbols = db.context().symbols();
            let filtered_values = time_module_provider(
                db,
                "executable_body_check.value_resolution",
                module_id,
                || {
                    nia_value_resolve::resolve_module_values_from_active_item_tree_with_associated_values_and_symbols(
                        &filtered_active_item_tree,
                        context.defs,
                        nia_value_resolve::ProgramDefsContext {
                            defs: Some(&program_defs),
                            graph: Some(&db.get(ModuleGraphQuery)),
                        },
                        &public_surfaces.surfaces,
                        using_scope.as_ref(),
                        Some(&associated_values),
                        Some(&symbols),
                    )
                },
            );
            let filtered_locals = time_module_provider(
                db,
                "executable_body_check.local_resolution",
                module_id,
                || {
                    nia_local_resolve::resolve_module_locals_from_filtered_active_item_tree_with_origins_and_symbols(
                        &filtered_active_item_tree,
                        &context.active_item_tree,
                        context.defs,
                        &filtered_values,
                        Some(context.source_version),
                        context.origins,
                        &symbols,
                    )
                },
            );
            let filtered_semantic_uses = time_module_provider(
                db,
                "executable_body_check.semantic_uses",
                module_id,
                || {
                    let needed_const_exprs = needed_const_exprs_for_active_item_tree(
                        &db.context().type_store,
                        &filtered_active_item_tree,
                        context.lowered,
                    );
                    let const_expr_value_resolution = time_module_provider(
                        db,
                        "executable_body_check.const_expr_value_resolution",
                        module_id,
                        || {
                            nia_value_resolve::resolve_module_values_from_exprs_with_associated_values_and_symbols(
                                context.lowered.const_exprs.iter().filter_map(|(id, expr)| {
                                    needed_const_exprs.contains(id).then_some(expr.clone())
                                }),
                                context.defs,
                                nia_value_resolve::ProgramDefsContext {
                                    defs: Some(&program_defs),
                                    graph: Some(&db.get(ModuleGraphQuery)),
                                },
                                &public_surfaces.surfaces,
                                using_scope.as_ref(),
                                Some(&associated_values),
                                Some(&symbols),
                            )
                        },
                    );
                    semantic_use_table_from_resolution_inputs_with_const_expr_values(
                        SemanticUseInputs {
                            module_id,
                            type_store: &db.context().type_store,
                            active_item_tree: &filtered_active_item_tree,
                            values: &filtered_values,
                            const_expr_values: Some(&const_expr_value_resolution),
                            const_expr_value_ids: Some(&needed_const_exprs),
                            locals: &filtered_locals,
                            type_resolution: context.type_resolution,
                            type_lowering: context.lowered,
                        },
                    )
                },
            );
            BodyCheckResolutionInputs {
                active_item_tree: filtered_active_item_tree,
                values: Arc::new(filtered_values),
                locals: Arc::new(filtered_locals),
                semantic_uses: Arc::new(filtered_semantic_uses),
            }
        }
    }
}

pub(super) fn full_body_check_resolution_inputs(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> BodyCheckResolutionInputs {
    let source_version = db.query(ModuleSourceVersionQuery(module_id));
    let origins = db.query(ModuleOriginsQuery(module_id));
    let active_item_tree = db.get(FullActiveModuleItemTreeQuery(module_id));
    let defs = db.get(FullModuleDefsQuery(module_id));
    let type_resolution = db.get(TypeResolutionQuery(module_id));
    let lowered = db.get(TypeLoweringQuery(module_id));
    body_check_resolution_inputs_for_filter(
        db,
        module_id,
        nia_body_check::BodyCheckFilter::All,
        BodyCheckResolutionContext {
            source_version,
            origins: &origins,
            active_item_tree,
            defs: &defs,
            type_resolution: &type_resolution,
            lowered: &lowered,
        },
    )
}

#[derive(Clone)]
pub(in crate::query) struct BodyCheckResolutionInputs {
    pub(super) active_item_tree: Arc<ActiveModuleItemTree>,
    pub(super) values: Arc<ValueResolution>,
    pub(super) locals: Arc<LocalResolution>,
    pub(super) semantic_uses: Arc<nia_sema_ir::SemanticUseTable>,
}

pub(in crate::query) struct BodyCheckWithResolutionInputs {
    pub(in crate::query) body_check: nia_body_check::BodyCheck,
    pub(in crate::query) inputs: BodyCheckResolutionInputs,
    pub(in crate::query) const_eval: Option<ConstCheck>,
}

pub(super) struct BodyCheckResolutionContext<'a> {
    pub(super) source_version: nia_source::SourceVersion,
    pub(super) origins: &'a nia_node_id::NodeOriginTable,
    pub(super) active_item_tree: Arc<ActiveModuleItemTree>,
    pub(super) defs: &'a DefCollection,
    pub(super) type_resolution: &'a TypeResolution,
    pub(super) lowered: &'a TypeLowering,
}

pub(super) fn body_local_item_signatures(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    lowered: &TypeLowering,
) -> ItemSignatures {
    let defs = db.get(FullModuleDefsQuery(module_id));
    let functions = collect_body_signature_subset(
        db,
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
        &defs,
        lowered,
    );
    let extension_functions = collect_body_signature_subset(
        db,
        module_id,
        nia_item_tree::SignatureItemSet::ExtensionFunctions,
        &defs,
        lowered,
    );
    let values = collect_body_signature_subset(
        db,
        module_id,
        nia_item_tree::SignatureItemSet::Values,
        &defs,
        lowered,
    );
    let types = collect_body_signature_subset(
        db,
        module_id,
        nia_item_tree::SignatureItemSet::Types,
        &defs,
        lowered,
    );
    let traits = collect_body_signature_subset(
        db,
        module_id,
        nia_item_tree::SignatureItemSet::Traits,
        &defs,
        lowered,
    );
    let mut function_signatures = functions.functions;
    function_signatures.extend(extension_functions.functions);
    function_signatures.extend(traits.functions.clone());
    let mut global_signatures = values.globals;
    global_signatures.extend(functions.globals);
    global_signatures.extend(extension_functions.globals);
    global_signatures.extend(traits.globals);
    let mut const_signatures = values.consts;
    const_signatures.extend(functions.consts);
    const_signatures.extend(extension_functions.consts);
    const_signatures.extend(traits.consts);
    let mut diagnostics = functions.diagnostics;
    diagnostics.extend(extension_functions.diagnostics);
    diagnostics.extend(values.diagnostics);
    diagnostics.extend(types.diagnostics);
    diagnostics.extend(traits.diagnostics);
    ItemSignatures {
        functions: function_signatures,
        structs: types.structs,
        unions: types.unions,
        traits: traits.traits,
        trait_impls: traits.trait_impls,
        enums: types.enums,
        type_aliases: types.type_aliases,
        globals: global_signatures,
        consts: const_signatures,
        diagnostics,
    }
}

fn collect_body_signature_subset(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
    defs: &DefCollection,
    lowered: &TypeLowering,
) -> ItemSignatures {
    let active_item_tree = db.get(SignatureItemTreeQuery(module_id, set));
    let symbols = db.context().symbols();
    nia_item_signatures::collect_item_signatures(nia_item_signatures::ItemSignatureInput {
        source: nia_item_signatures::ItemSignatureSource::ActiveItemTree(&active_item_tree),
        defs,
        lowered,
        type_store: db.context().type_store(),
        symbols: Some(&symbols),
    })
}
