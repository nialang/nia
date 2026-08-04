use super::*;

pub(super) fn with_type_signature_const_input<T>(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    non_function_signatures_override: Option<&ProgramExecutableNonFunctionSignatures>,
    f: impl FnOnce(nia_const_check::ConstInput<'_>, &ConstModuleLowering) -> T,
) -> QueryResult<T> {
    with_signature_const_input(db, module_id, non_function_signatures_override, f)
}

fn with_signature_const_input<T>(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    non_function_signatures_override: Option<&ProgramExecutableNonFunctionSignatures>,
    f: impl FnOnce(nia_const_check::ConstInput<'_>, &ConstModuleLowering) -> T,
) -> QueryResult<T> {
    let module = db.get(SignatureConstModuleQuery(module_id))?;
    let active_item_tree = db.get(SignatureConstItemTreeQuery(module_id))?;
    let defs = module_defs_semantic(db, module_id)?;
    let type_lowering = db.get(SignatureConstTypeLoweringQuery(module_id))?;
    let values = signature_const_value_resolution(db, module_id, &active_item_tree)?;
    let locals = empty_local_resolution(db.context().node_store());
    let type_resolution = db.get(SignatureConstTypeResolutionQuery(module_id))?;
    let type_normalization = db.get(SignatureConstTypeNormalizationQuery(module_id))?;
    let semantic_uses = signature_semantic_use_table_from_resolution_inputs(
        db.context().node_store(),
        &db.context().type_store,
        module_id,
        &active_item_tree,
        &values,
        &type_resolution,
        &type_lowering,
    );
    let signatures = db.get(SignatureConstItemSignaturesQuery(module_id))?;
    let source_path = db.get(ModulePathQuery(module_id))?;
    let query_failure = RefCell::new(None);
    let program_module = |module_id| {
        capture_query_failure(&query_failure, db.get(SignatureConstModuleQuery(module_id)))
            .map(|module| Arc::clone(&module.module))
    };
    let program_source_path = |module_id| {
        capture_query_failure(&query_failure, db.get(ModulePathQuery(module_id)))
            .map(|path| path.as_ref().clone())
    };
    let program_defs =
        |module_id| capture_query_failure(&query_failure, module_defs_semantic(db, module_id));
    let program_type_normalization = |module_id| {
        capture_query_failure(
            &query_failure,
            signature_type_normalization_semantic(
                db,
                module_id,
                nia_item_tree::SignatureItemSet::Types,
            ),
        )
    };
    let local_trait_impls = if non_function_signatures_override.is_none() {
        Some(db.get(VisibleTraitImplsQuery(module_id))?)
    } else {
        None
    };
    let trait_impls_for_module = |requested_module_id| {
        if requested_module_id == module_id {
            return non_function_signatures_override
                .map(|signatures| signatures.trait_impls.clone())
                .or_else(|| {
                    local_trait_impls
                        .as_ref()
                        .map(|signatures| signatures.trait_impls.clone())
                });
        }
        if let Some(signatures) = non_function_signatures_override {
            return Some(signatures.trait_impls.clone());
        }
        capture_query_failure(
            &query_failure,
            db.get(VisibleTraitImplsQuery(requested_module_id)),
        )
        .map(|signatures| signatures.trait_impls.clone())
    };
    let program_is_enum = |def_id: GlobalDefId| {
        non_function_signatures_override
            .is_some_and(|signatures| signatures.enums.contains_key(&def_id))
            || capture_query_failure(
                &query_failure,
                db.get(SignatureItemSignaturesQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                )),
            )
            .is_some_and(|signatures| signatures.semantic.enums.contains_key(&def_id.def_id))
    };
    let item_signatures_for_module = |module_id| {
        capture_query_failure(
            &query_failure,
            signature_item_signatures_semantic(
                db,
                module_id,
                nia_item_tree::SignatureItemSet::Types,
            ),
        )
    };
    let value_signatures_for_module = |module_id| {
        capture_query_failure(
            &query_failure,
            signature_item_signatures_semantic(
                db,
                module_id,
                nia_item_tree::SignatureItemSet::Values,
            ),
        )
    };
    let local_visible_extensions = db.get(VisibleExtensionsQuery(module_id))?;
    let visible_extensions_for_module = |requested_module_id| {
        if requested_module_id == module_id {
            return Some(local_visible_extensions.methods.clone());
        }
        capture_query_failure(
            &query_failure,
            db.get(VisibleExtensionsQuery(requested_module_id)),
        )
        .map(|extensions| extensions.methods.clone())
    };
    let target = db.get(CompilerTargetQuery)?;
    let symbols = db.context().symbols();
    let input = nia_const_check::ConstInput {
        type_store: &db.context().type_store,
        module: &module.module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        symbols: &symbols,
        lowered: &type_lowering,
        signatures: &signatures,
        normalization: &type_normalization,
        target: &target,
        source_path: &source_path,
        program: nia_const_check::ConstProgramContext {
            module: Some(&program_module),
            source_path: Some(&program_source_path),
            defs: Some(&program_defs),
            type_normalizations: Some(&program_type_normalization),
            signatures: Some(&item_signatures_for_module),
            value_signatures: Some(&value_signatures_for_module),
            const_values: None,
            global_initializer: None,
            program_is_enum: Some(&program_is_enum),
            trait_impls_for_module: Some(&trait_impls_for_module),
            visible_extensions: Some(&visible_extensions_for_module),
        },
    };
    let output = f(input, &module);
    match query_failure.into_inner() {
        Some(error) => Err(error),
        None => Ok(output),
    }
}

pub(super) fn provide_signature_const_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ConstModuleLowering> {
    let active_item_tree = db.get(SignatureConstItemTreeQuery(module_id))?;
    let defs = module_defs_semantic(db, module_id)?;
    let type_lowering = db.get(SignatureConstTypeLoweringQuery(module_id))?;
    let values = signature_const_value_resolution(db, module_id, &active_item_tree)?;
    let locals = empty_local_resolution(db.context().node_store());
    let type_resolution = db.get(SignatureConstTypeResolutionQuery(module_id))?;
    let semantic_uses = signature_semantic_use_table_from_resolution_inputs(
        db.context().node_store(),
        &db.context().type_store,
        module_id,
        &active_item_tree,
        &values,
        &type_resolution,
        &type_lowering,
    );
    let signatures = db.get(SignatureConstItemSignaturesQuery(module_id))?;
    let source_path = db.get(ModulePathQuery(module_id))?;
    let symbols = db.context().symbols();
    Ok(nia_const_check::lower_module_const(
        nia_const_check::ConstModuleInput {
            active_item_tree: &active_item_tree,
            defs: &defs,
            signatures: &signatures,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
            symbols: &symbols,
            const_exprs: &type_lowering.const_exprs,
            source_path: &source_path,
        },
    ))
}

pub(super) fn signature_const_module_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<Arc<ConstModuleLowering>> {
    db.get(SignatureConstModuleQuery(module_id))
}

pub(super) fn signature_const_array_lengths(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    non_function_signatures_override: Option<&ProgramExecutableNonFunctionSignatures>,
) -> QueryResult<nia_const_check::ConstArrayLengths> {
    with_type_signature_const_input(
        db,
        module_id,
        non_function_signatures_override,
        |input, module| {
            let mut array_lengths = nia_const_check::compute_module_const_array_lengths(input);
            array_lengths.diagnostics.extend(module.diagnostics.clone());
            array_lengths
        },
    )
}

pub(super) fn signature_const_values(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    non_function_signatures_override: Option<&ProgramExecutableNonFunctionSignatures>,
) -> QueryResult<nia_const_check::ConstValues> {
    let array_lengths =
        signature_const_array_lengths(db, module_id, non_function_signatures_override)?;
    let enum_values = with_type_signature_const_input(
        db,
        module_id,
        non_function_signatures_override,
        |input, module| {
            let mut enum_values =
                nia_const_check::compute_module_const_enum_values(input, array_lengths.clone());
            enum_values.diagnostics.extend(module.diagnostics.clone());
            enum_values
        },
    )?;
    with_type_signature_const_input(
        db,
        module_id,
        non_function_signatures_override,
        |input, module| {
            let mut values =
                nia_const_check::compute_module_const_values(input, array_lengths, enum_values);
            values.diagnostics.extend(module.diagnostics.clone());
            values
        },
    )
}

fn signature_const_value_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    active_item_tree: &ActiveModuleItemTree,
) -> QueryResult<ValueResolution> {
    let type_lowering = db.get(SignatureConstTypeLoweringQuery(module_id))?;
    let needed_const_exprs = needed_const_exprs_for_active_item_tree(
        &db.context().type_store,
        active_item_tree,
        &type_lowering,
    );
    let mut exprs = type_lowering
        .const_exprs
        .iter()
        .filter_map(|(id, expr)| needed_const_exprs.contains(id).then_some(expr.clone()))
        .collect::<Vec<_>>();
    collect_enum_discriminant_exprs(active_item_tree, &mut exprs);
    let has_const_provider_values = active_item_tree
        .items
        .iter()
        .any(item_tree_node_has_const_provider_values);
    if exprs.is_empty() && !has_const_provider_values {
        return Ok(empty_value_resolution(db.context().node_store()));
    }
    let defs = module_defs_semantic(db, module_id)?;
    let public_surfaces = db.get(PublicSurfacesQuery)?;
    let using_scope = db.get(ModuleUsingScopeQuery(module_id))?;
    let graph = db.get(ModuleGraphQuery)?;
    let visible_extensions = || db.get(VisibleExtensionsQuery(module_id));
    let associated_values =
        LazyAssociatedValueResolver::new(&db.context().type_store, &visible_extensions);
    let query_failure = RefCell::new(None);
    let program_defs =
        |module_id| capture_query_failure(&query_failure, module_defs_semantic(db, module_id));
    let symbols = db.context().symbols();
    let values =
        nia_value_resolve::resolve_module_values_from_active_item_tree_with_associated_values_and_symbols_in_store(
            active_item_tree,
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
    if exprs.is_empty() {
        return match query_failure
            .into_inner()
            .or_else(|| associated_values.take_failure())
        {
            Some(error) => Err(error),
            None => Ok(values),
        };
    }
    let const_expr_values =
        nia_value_resolve::resolve_module_values_from_exprs_with_associated_values_and_symbols_in_store(
            exprs,
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
    let values = values.extend(const_expr_values);
    match query_failure
        .into_inner()
        .or_else(|| associated_values.take_failure())
    {
        Some(error) => Err(error),
        None => Ok(values),
    }
}

fn signature_semantic_use_table_from_resolution_inputs(
    node_store: &nia_node_id::NodeStore,
    type_store: &nia_ty::TypeStore,
    module_id: ModuleId,
    active_item_tree: &ActiveModuleItemTree,
    values: &ValueResolution,
    type_resolution: &TypeResolution,
    type_lowering: &TypeLowering,
) -> nia_sema_ir::SemanticUseTable {
    let empty_locals = empty_local_resolution(node_store);
    semantic_use_table_from_resolution_inputs_with_const_expr_values(SemanticUseInputs {
        module_id,
        node_store,
        type_store,
        active_item_tree,
        values,
        const_expr_values: None,
        const_expr_value_ids: None,
        locals: &empty_locals,
        type_resolution,
        type_lowering,
    })
}

fn collect_enum_discriminant_exprs(
    active_item_tree: &ActiveModuleItemTree,
    out: &mut Vec<nia_ast::Expr>,
) {
    for item in &active_item_tree.items {
        if let nia_item_tree::ItemTreeNodeKind::Enum(item_enum) = &item.kind {
            out.extend(
                item_enum
                    .variants
                    .iter()
                    .filter_map(|variant| variant.value.clone()),
            );
        }
    }
}

fn item_tree_node_has_const_provider_values(item: &nia_item_tree::ItemTreeNode) -> bool {
    match &item.kind {
        nia_item_tree::ItemTreeNodeKind::Binding(binding) => {
            binding.is_const() && binding.value.is_some()
        }
        nia_item_tree::ItemTreeNodeKind::Function(function) => {
            function.is_const && function.body.is_some()
        }
        nia_item_tree::ItemTreeNodeKind::Extend(extend) => {
            extend.associated_values.iter().any(|associated_value| {
                associated_value.binding.is_const() && associated_value.binding.value.is_some()
            }) || extend
                .methods
                .iter()
                .any(|method| method.function.is_const && method.function.body.is_some())
        }
        _ => false,
    }
}

fn empty_value_resolution(node_store: &nia_node_id::NodeStore) -> ValueResolution {
    ValueResolution::with_store(node_store)
}

fn empty_local_resolution(node_store: &nia_node_id::NodeStore) -> LocalResolution {
    LocalResolution::with_store(node_store)
}

pub(super) fn signature_layouts_for_types(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    non_function_signatures_override: Option<&ProgramExecutableNonFunctionSignatures>,
) -> QueryResult<nia_layout::Layouts> {
    time_module_provider(db, "signature_layouts", module_id, || {
        let defs = module_defs_semantic(db, module_id)?;
        let active_item_tree = db.get(SignatureItemTreeQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))?;
        let type_lowering = db.get(SignatureTypeLoweringQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))?;
        let type_normalization = db.get(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))?;
        let item_signatures = db.get(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))?;
        let target = compiler_target_data_layout(db)?;
        let query_failure = RefCell::new(None);
        let program_struct = |def_id: GlobalDefId| {
            capture_query_failure(
                &query_failure,
                db.get(SignatureItemSignaturesQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                )),
            )
            .and_then(|signatures| {
                signatures
                    .semantic
                    .structs
                    .get(&def_id.def_id)
                    .cloned()
                    .map(|signature| ProgramStructSignature { signature })
            })
        };
        let program_union = |def_id: GlobalDefId| {
            capture_query_failure(
                &query_failure,
                db.get(SignatureItemSignaturesQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                )),
            )
            .and_then(|signatures| {
                signatures
                    .semantic
                    .unions
                    .get(&def_id.def_id)
                    .cloned()
                    .map(|signature| ProgramUnionSignature { signature })
            })
        };
        let program_enum = |def_id: GlobalDefId| {
            capture_query_failure(
                &query_failure,
                db.get(SignatureItemSignaturesQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                )),
            )
            .and_then(|signatures| {
                signatures
                    .semantic
                    .enums
                    .get(&def_id.def_id)
                    .cloned()
                    .map(|signature| ProgramEnumSignature { signature })
            })
        };
        let program_type_alias = |def_id: GlobalDefId| {
            capture_query_failure(
                &query_failure,
                db.get(SignatureItemSignaturesQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                )),
            )
            .and_then(|signatures| {
                signatures
                    .semantic
                    .type_aliases
                    .get(&def_id.def_id)
                    .cloned()
                    .map(|signature| ProgramTypeAliasSignature { signature })
            })
        };
        let array_lengths = with_type_signature_const_input(
            db,
            module_id,
            non_function_signatures_override,
            |input, module| {
                let mut array_lengths = nia_const_check::compute_module_const_array_lengths(input);
                array_lengths.diagnostics.extend(module.diagnostics.clone());
                array_lengths
            },
        )?;
        let local_array_lengths = |id| array_lengths.values.get(&id).copied();
        let program_array_lengths = |id: nia_ids::GlobalConstExprId| {
            capture_query_failure(
                &query_failure,
                with_type_signature_const_input(
                    db,
                    id.module_id,
                    non_function_signatures_override,
                    |input, module| {
                        let mut array_lengths =
                            nia_const_check::compute_module_const_array_lengths(input);
                        array_lengths.diagnostics.extend(module.diagnostics.clone());
                        array_lengths
                    },
                ),
            )
            .and_then(|array_lengths| array_lengths.values.get(&id).copied())
        };
        let symbols = db.context().symbols();
        let roots = time_module_provider(db, "signature_layouts.roots", module_id, || {
            signature_layout_roots(
                &db.context().type_store,
                module_id,
                &item_signatures.semantic,
                &program_struct,
                &program_union,
                type_lowering
                    .semantic
                    .versioned_type_uses_from_active_item_tree(&active_item_tree)
                    .into_iter()
                    .map(|(_, ty)| ty),
            )
        });
        let layouts = nia_layout::compute_layouts_for_roots_with_program_context(
            nia_layout::LayoutComputationInput {
                type_store: &db.context().type_store,
                defs: &defs,
                signatures: &item_signatures.semantic,
                root_types: &[],
                normalized: &type_normalization.semantic.normalized,
                array_lengths: &local_array_lengths,
                target,
                program: nia_layout::ProgramLayoutContext {
                    symbols: Some(&symbols),
                    array_lengths: Some(&program_array_lengths),
                    struct_: Some(&program_struct),
                    union: Some(&program_union),
                    enum_: Some(&program_enum),
                    type_alias: Some(&program_type_alias),
                    ..Default::default()
                },
            },
            nia_layout::LayoutRoots {
                types: &roots.types,
                structs: &roots.structs,
                unions: &roots.unions,
            },
        );
        match query_failure.into_inner() {
            Some(error) => Err(error),
            None => Ok(layouts),
        }
    })
}

fn signature_layout_roots(
    type_store: &nia_ty::TypeStore,
    module_id: ModuleId,
    signatures: &ItemSignatures,
    program_struct: &dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>,
    program_union: &dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>,
    type_uses: impl IntoIterator<Item = InternedTyId>,
) -> CollectedLayoutRoots {
    let mut roots =
        LayoutRootCollector::with_program(type_store, module_id, program_struct, program_union);
    for ty in type_uses {
        roots.add(ty);
    }
    for def_id in signatures.structs.keys().copied() {
        roots.add_struct(def_id);
    }
    for def_id in signatures.unions.keys().copied() {
        roots.add_union(def_id);
    }
    roots.finish()
}
