use super::*;

pub(super) fn with_type_signature_comptime_input<T>(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    non_function_signatures_override: Option<&ProgramExecutableNonFunctionSignatures>,
    f: impl FnOnce(nia_comptime_check::ComptimeInput<'_>, &ComptimeModuleLowering) -> T,
) -> T {
    with_signature_comptime_input(db, module_id, non_function_signatures_override, f)
}

fn with_signature_comptime_input<T>(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    non_function_signatures_override: Option<&ProgramExecutableNonFunctionSignatures>,
    f: impl FnOnce(nia_comptime_check::ComptimeInput<'_>, &ComptimeModuleLowering) -> T,
) -> T {
    let module = db.query(SignatureComptimeModuleQuery(module_id));
    let active_item_tree = db.query(SignatureComptimeItemTreeQuery(module_id));
    let defs = db.query_shared(ModuleDefsQuery(module_id));
    let type_lowering = db.query(SignatureComptimeTypeLoweringQuery(module_id));
    let values = signature_comptime_value_resolution(db, module_id, &active_item_tree);
    let locals = empty_local_resolution();
    let type_resolution = db.query(SignatureComptimeTypeResolutionQuery(module_id));
    let type_normalization = db.query(SignatureComptimeTypeNormalizationQuery(module_id));
    let semantic_uses = signature_semantic_use_table_from_resolution_inputs(
        module_id,
        &active_item_tree,
        &values,
        &type_resolution,
        &type_lowering,
    );
    let signatures = db.query(SignatureComptimeItemSignaturesQuery(module_id));
    let source_path = db.query(ModulePathQuery(module_id));
    let program_module = |module_id| Some(db.query(SignatureComptimeModuleQuery(module_id)).module);
    let program_source_path = |module_id| Some(db.query(ModulePathQuery(module_id)));
    let program_defs = |module_id| Some(db.query_shared(ModuleDefsQuery(module_id)));
    let program_type_normalization = |module_id| {
        Some(db.query(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Types,
        )))
    };
    let value_type_normalization = |module_id| {
        Some(db.query(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Values,
        )))
    };
    let trait_impls_for_module = |module_id| {
        if let Some(signatures) = non_function_signatures_override {
            return Some(signatures.trait_impls.clone());
        }
        Some(
            db.query(VisibleTraitImplsQuery(module_id))
                .trait_impls
                .clone(),
        )
    };
    let program_is_enum = |def_id: GlobalDefId| {
        non_function_signatures_override
            .is_some_and(|signatures| signatures.enums.contains_key(&def_id))
            || db
                .query_shared(SignatureItemSignaturesQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ))
                .enums
                .contains_key(&def_id.def_id)
    };
    let item_signatures_for_module = |module_id| {
        Some(db.query_shared(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Types,
        )))
    };
    let value_signatures_for_module = |module_id| {
        Some(db.query_shared(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Values,
        )))
    };
    let visible_extensions_for_module =
        |module_id| Some(db.query(VisibleExtensionsQuery(module_id)).methods.clone());
    let target = db.query(CompilerTargetQuery);
    let symbols = db.context().symbols();
    f(
        nia_comptime_check::ComptimeInput {
            module: &module.module,
            defs: &defs,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
            symbols: &symbols,
            lowered: &type_lowering,
            signatures: &signatures,
            interner: &type_normalization.interner,
            normalized: &type_normalization.normalized,
            target: &target,
            source_path: &source_path,
            program: nia_comptime_check::ComptimeProgramContext {
                module: Some(&program_module),
                source_path: Some(&program_source_path),
                defs: Some(&program_defs),
                type_normalizations: Some(&program_type_normalization),
                value_type_normalizations: Some(&value_type_normalization),
                signatures: Some(&item_signatures_for_module),
                value_signatures: Some(&value_signatures_for_module),
                comptime_values: None,
                global_initializer: None,
                program_is_enum: Some(&program_is_enum),
                trait_impls_for_module: Some(&trait_impls_for_module),
                visible_extensions: Some(&visible_extensions_for_module),
            },
        },
        &module,
    )
}

pub(super) fn provide_signature_comptime_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ComptimeModuleLowering {
    let active_item_tree = db.query(SignatureComptimeItemTreeQuery(module_id));
    let defs = db.query_shared(ModuleDefsQuery(module_id));
    let type_lowering = db.query(SignatureComptimeTypeLoweringQuery(module_id));
    let values = signature_comptime_value_resolution(db, module_id, &active_item_tree);
    let locals = empty_local_resolution();
    let type_resolution = db.query(SignatureComptimeTypeResolutionQuery(module_id));
    let semantic_uses = signature_semantic_use_table_from_resolution_inputs(
        module_id,
        &active_item_tree,
        &values,
        &type_resolution,
        &type_lowering,
    );
    let signatures = db.query(SignatureComptimeItemSignaturesQuery(module_id));
    let source_path = db.query(ModulePathQuery(module_id));
    let symbols = db.context().symbols();
    nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
        active_item_tree: &active_item_tree,
        defs: &defs,
        signatures: &signatures,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        symbols: &symbols,
        const_exprs: &type_lowering.const_exprs,
        source_path: &source_path,
    })
}

pub(super) fn signature_comptime_module_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ComptimeModuleLowering {
    db.query(SignatureComptimeModuleQuery(module_id))
}

pub(super) fn signature_comptime_array_lengths(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    non_function_signatures_override: Option<&ProgramExecutableNonFunctionSignatures>,
) -> nia_comptime_check::ComptimeArrayLengths {
    with_type_signature_comptime_input(
        db,
        module_id,
        non_function_signatures_override,
        |input, module| {
            let mut array_lengths =
                nia_comptime_check::compute_module_comptime_array_lengths(input);
            array_lengths.diagnostics.extend(module.diagnostics.clone());
            array_lengths
        },
    )
}

pub(super) fn signature_comptime_values(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    non_function_signatures_override: Option<&ProgramExecutableNonFunctionSignatures>,
) -> nia_comptime_check::ComptimeValues {
    let array_lengths =
        signature_comptime_array_lengths(db, module_id, non_function_signatures_override);
    let enum_values = with_type_signature_comptime_input(
        db,
        module_id,
        non_function_signatures_override,
        |input, module| {
            let mut enum_values = nia_comptime_check::compute_module_comptime_enum_values(
                input,
                array_lengths.clone(),
            );
            enum_values.diagnostics.extend(module.diagnostics.clone());
            enum_values
        },
    );
    with_type_signature_comptime_input(
        db,
        module_id,
        non_function_signatures_override,
        |input, module| {
            let mut values = nia_comptime_check::compute_module_comptime_values(
                input,
                array_lengths,
                enum_values,
            );
            values.diagnostics.extend(module.diagnostics.clone());
            values
        },
    )
}

fn signature_comptime_value_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    active_item_tree: &ActiveModuleItemTree,
) -> ValueResolution {
    let type_lowering = db.query(SignatureComptimeTypeLoweringQuery(module_id));
    let needed_const_exprs =
        needed_const_exprs_for_active_item_tree(active_item_tree, &type_lowering);
    let mut exprs = type_lowering
        .const_exprs
        .iter()
        .filter_map(|(id, expr)| needed_const_exprs.contains(id).then_some(expr.clone()))
        .collect::<Vec<_>>();
    collect_enum_discriminant_exprs(active_item_tree, &mut exprs);
    let has_comptime_provider_values = active_item_tree
        .items
        .iter()
        .any(item_tree_node_has_comptime_provider_values);
    if exprs.is_empty() && !has_comptime_provider_values {
        return empty_value_resolution();
    }
    let defs = db.query_shared(ModuleDefsQuery(module_id));
    let public_surfaces = db.query(PublicSurfacesQuery);
    let using_scope = db.query(ModuleUsingScopeQuery(module_id));
    let visible_extensions = || db.query(VisibleExtensionsQuery(module_id));
    let associated_values = LazyAssociatedValueResolver::new(&visible_extensions);
    let program_defs = |module_id| Some(db.query_shared(ModuleDefsQuery(module_id)));
    let symbols = db.context().symbols();
    let mut values =
        nia_value_resolve::resolve_module_values_from_active_item_tree_with_associated_values_and_symbols(
            active_item_tree,
            &defs,
            nia_value_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&db.query_shared(ModuleGraphQuery)),
            },
            &public_surfaces.surfaces,
            &using_scope,
            Some(&associated_values),
            Some(&symbols),
        );
    if exprs.is_empty() {
        return values;
    }
    let const_expr_values =
        nia_value_resolve::resolve_module_values_from_exprs_with_associated_values_and_symbols(
            exprs,
            &defs,
            nia_value_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&db.query_shared(ModuleGraphQuery)),
            },
            &public_surfaces.surfaces,
            &using_scope,
            Some(&associated_values),
            Some(&symbols),
        );
    values.node_names.extend(const_expr_values.node_names);
    values
        .node_qualified_values
        .extend(const_expr_values.node_qualified_values);
    values
        .node_builtin_associated_values
        .extend(const_expr_values.node_builtin_associated_values);
    values
        .node_variant_enums
        .extend(const_expr_values.node_variant_enums);
    values
        .node_qualified_type_prefixes
        .extend(const_expr_values.node_qualified_type_prefixes);
    values.diagnostics.extend(const_expr_values.diagnostics);
    values
}

fn signature_semantic_use_table_from_resolution_inputs(
    module_id: ModuleId,
    active_item_tree: &ActiveModuleItemTree,
    values: &ValueResolution,
    type_resolution: &TypeResolution,
    type_lowering: &TypeLowering,
) -> nia_sema_ir::SemanticUseTable {
    let empty_locals = empty_local_resolution();
    semantic_use_table_from_resolution_inputs_with_const_expr_values(
        module_id,
        active_item_tree,
        values,
        None,
        None,
        &empty_locals,
        type_resolution,
        type_lowering,
    )
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

fn item_tree_node_has_comptime_provider_values(item: &nia_item_tree::ItemTreeNode) -> bool {
    match &item.kind {
        nia_item_tree::ItemTreeNodeKind::Binding(binding) => {
            binding.is_comptime && binding.value.is_some()
        }
        nia_item_tree::ItemTreeNodeKind::Function(function) => {
            function.is_comptime && function.body.is_some()
        }
        nia_item_tree::ItemTreeNodeKind::Extend(extend) => {
            extend.associated_values.iter().any(|associated_value| {
                associated_value.binding.is_comptime && associated_value.binding.value.is_some()
            }) || extend
                .methods
                .iter()
                .any(|method| method.function.is_comptime && method.function.body.is_some())
        }
        _ => false,
    }
}

fn empty_value_resolution() -> ValueResolution {
    ValueResolution {
        node_names: HashMap::new(),
        node_qualified_values: HashMap::new(),
        node_builtin_associated_values: HashMap::new(),
        node_variant_enums: HashMap::new(),
        node_qualified_type_prefixes: HashMap::new(),
        diagnostics: Vec::new(),
    }
}

fn empty_local_resolution() -> LocalResolution {
    LocalResolution {
        locals: nia_local_resolve::LocalMap::default(),
        node_local_defs: HashMap::new(),
        node_uses: HashMap::new(),
        diagnostics: Vec::new(),
    }
}

pub(super) fn signature_layouts_for_types(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    non_function_signatures_override: Option<&ProgramExecutableNonFunctionSignatures>,
) -> nia_layout::Layouts {
    time_module_provider(db, "signature_layouts", module_id, || {
        let defs = db.query_shared(ModuleDefsQuery(module_id));
        let active_item_tree = db.query_shared(SignatureItemTreeQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Types,
        ));
        let type_lowering = db.query_shared(SignatureTypeLoweringQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Types,
        ));
        let type_normalization = db.query(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Types,
        ));
        let item_signatures = db.query_shared(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Types,
        ));
        let program_struct = |def_id: GlobalDefId| {
            db.query_shared(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ))
            .structs
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramStructSignature {
                signature,
                interner: signature_type_interner(
                    db,
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ),
            })
        };
        let program_union = |def_id: GlobalDefId| {
            db.query_shared(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ))
            .unions
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramUnionSignature {
                signature,
                interner: signature_type_interner(
                    db,
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ),
            })
        };
        let program_enum = |def_id: GlobalDefId| {
            db.query_shared(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ))
            .enums
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramEnumSignature {
                signature,
                interner: signature_type_interner(
                    db,
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ),
            })
        };
        let program_type_alias = |def_id: GlobalDefId| {
            db.query_shared(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ))
            .type_aliases
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramTypeAliasSignature {
                signature,
                interner: signature_type_interner(
                    db,
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ),
            })
        };
        let array_lengths = with_type_signature_comptime_input(
            db,
            module_id,
            non_function_signatures_override,
            |input, module| {
                let mut array_lengths =
                    nia_comptime_check::compute_module_comptime_array_lengths(input);
                array_lengths.diagnostics.extend(module.diagnostics.clone());
                array_lengths
            },
        );
        let local_array_lengths = |id| array_lengths.values.get(&id).copied();
        let program_array_lengths = |id: nia_ids::GlobalConstExprId| {
            with_type_signature_comptime_input(
                db,
                id.module_id,
                non_function_signatures_override,
                |input, module| {
                    let mut array_lengths =
                        nia_comptime_check::compute_module_comptime_array_lengths(input);
                    array_lengths.diagnostics.extend(module.diagnostics.clone());
                    array_lengths
                },
            )
            .values
            .get(&id)
            .copied()
        };
        let (layout_interner, roots) =
            time_module_provider(db, "signature_layouts.roots", module_id, || {
                let mut layout_interner = type_normalization.interner.clone();
                let roots = signature_layout_roots(
                    &mut layout_interner,
                    &item_signatures,
                    &program_struct,
                    &program_union,
                    type_lowering
                        .versioned_type_uses_from_active_item_tree(&active_item_tree)
                        .into_iter()
                        .map(|(_, ty)| ty),
                );
                (layout_interner, roots)
            });
        let symbols = db.context().symbols();
        nia_layout::compute_layouts_for_roots_with_program_context(
            nia_layout::LayoutComputationInput {
                defs: &defs,
                interner: &layout_interner,
                signatures: &item_signatures,
                normalized: &type_normalization.normalized,
                array_lengths: &local_array_lengths,
                target: nia_layout::TargetDataLayout::LP64,
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
        )
    })
}

fn signature_layout_roots(
    interner: &mut nia_ty::TyInterner,
    signatures: &ItemSignatures,
    program_struct: &dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>,
    program_union: &dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>,
    type_uses: impl IntoIterator<Item = InternedTyId>,
) -> CollectedLayoutRoots {
    let mut roots = LayoutRootCollector::with_program(interner, program_struct, program_union);
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
