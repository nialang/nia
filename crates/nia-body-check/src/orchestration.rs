// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

/// Checks active module bodies with local signatures and default products.
pub fn check_module_bodies(
    type_store: &nia_ty::TypeStore,
    module: &Module,
    defs: &DefCollection,
    values: &ValueResolution,
    locals: &LocalResolution,
    lowered: &TypeLowering,
    signatures: &ItemSignatures,
) -> BodyCheck {
    let target = TargetConfig::host();
    let empty_normalization = TypeNormalization {
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let Some(target_layout) = target_data_layout(&target) else {
        return body_check_target_layout_error(&target);
    };
    let layouts = nia_layout::compute_layouts(type_store, defs, signatures, target_layout);
    let empty_const_module = ResolvedConstModule::default();
    let empty_extensions = VisibleExtensionMethods::default();
    let empty_program_extension_methods = ExtensionMethods::default();
    let empty_const_values = HashMap::new();
    let empty_typed_const_values = HashMap::new();
    let empty_array_lengths = HashMap::new();
    let empty_const = BodyConst {
        values: &empty_const_values,
        typed_values: &empty_typed_const_values,
        array_lengths: &empty_array_lengths,
    };
    let source_path = SourcePath::new("main.nia");
    let symbols = SymbolTable::new();
    let item_tree = ModuleItemTree::from_module(module);
    let active_item_tree =
        ActiveModuleItemTree::new(item_tree.active_items_without_const(), Default::default());
    let semantic_uses = semantic_use_table_for_body_input(
        defs.module_id,
        values,
        locals,
        lowered,
        &active_item_tree,
    );
    let input = BodyCheckInput {
        type_store,
        source_version: None,
        source_path: &source_path,
        symbols: &symbols,
        origins: &NodeOriginTable::default(),
        active_item_tree: &active_item_tree,
        defs,
        values,
        locals,
        semantic_uses: &semantic_uses,
        lowered,
        signatures: BodyLocalSignatures::from_item_signatures(signatures),
        const_signatures: signatures,
        normalization: &empty_normalization,
        seed: None,
        target: &target,
        const_eval: empty_const,
        const_module: &empty_const_module,
        layouts: &layouts,
        extensions: &empty_extensions,
        lazy_extensions: None,
        program_extension_methods: &empty_program_extension_methods,
        program: BodyProgramContext::empty(),
        program_signatures: ProgramSignatureContext::empty(),
        program_const: ProgramConstMaps::empty(),
        function_scope: FunctionCheckScope::LocalModule,
        filter: BodyCheckFilter::All,
        product: BodyCheckProduct::Full,
        prechecked: None,
    };
    let mut checked = check_module_bodies_with_program_signatures_and_layouts_with_timings(
        input,
        nia_timing::TimingMode::Off,
    );
    Arc::make_mut(&mut checked.diagnostics).extend(layouts.diagnostics);
    checked
}

/// Checks bodies using caller-provided layouts and product/filter settings.
pub fn check_module_bodies_with_layouts(input: BodyCheckInput<'_>) -> BodyCheck {
    check_module_bodies_with_program_signatures_and_layouts(input)
}

/// Checks bodies against program-wide signatures without custom layouts.
pub fn check_module_bodies_with_program_signatures(
    input: BodyCheckWithProgramSignaturesInput<'_>,
) -> BodyCheck {
    let root_types = input.signatures.type_roots();
    let array_lengths = |id| input.const_eval.array_lengths.get(&id).copied();
    let Some(target_layout) = target_data_layout(input.target) else {
        return body_check_target_layout_error(input.target);
    };
    let layouts =
        nia_layout::compute_layouts_with_program_context(nia_layout::LayoutComputationInput {
            type_store: input.type_store,
            defs: input.defs,
            signatures: input.signatures,
            root_types: &root_types,
            normalized: &input.normalization.normalized,
            array_lengths: &array_lengths,
            target: target_layout,
            program: nia_layout::ProgramLayoutContext::default(),
        });
    let mut checked = check_module_bodies_with_layouts(BodyCheckInput {
        type_store: input.type_store,
        source_version: input.source_version,
        source_path: input.source_path,
        symbols: input.symbols,
        origins: input.origins,
        active_item_tree: input.active_item_tree,
        defs: input.defs,
        values: input.values,
        locals: input.locals,
        semantic_uses: input.semantic_uses,
        lowered: input.lowered,
        signatures: BodyLocalSignatures::from_item_signatures(input.signatures),
        const_signatures: input.signatures,
        normalization: input.normalization,
        seed: None,
        target: input.target,
        const_eval: input.const_eval,
        const_module: input.const_module,
        layouts: &layouts,
        extensions: input.extensions,
        lazy_extensions: None,
        program_extension_methods: input.program_extension_methods,
        program: input.program,
        program_signatures: input.program_signatures,
        function_scope: input.function_scope,
        program_const: ProgramConstMaps::empty(),
        filter: BodyCheckFilter::All,
        product: BodyCheckProduct::Full,
        prechecked: None,
    });
    Arc::make_mut(&mut checked.diagnostics).extend(layouts.diagnostics);
    checked
}

fn target_data_layout(target: &TargetConfig) -> Option<nia_layout::TargetDataLayout> {
    nia_layout::TargetDataLayout::from_pointer_width(target.pointer_width)
}

fn body_check_target_layout_error(target: &TargetConfig) -> BodyCheck {
    let diagnostic = Diagnostic::user_error_at(
        codes::TARGET_CONFIG,
        Span::new(0, 0),
        format!(
            "body checking requires a supported target pointer width, got {}",
            target.pointer_width
        ),
    );
    BodyCheck {
        ir: Arc::new(BodyIr {
            function_bodies: HashMap::new(),
            global_inits: HashMap::new(),
        }),
        facts: Arc::new(SemanticFacts::default()),
        static_init_refs: HashMap::new(),
        checked_functions: HashSet::new(),
        provider_demands: Arc::new(HashSet::new()),
        provider_demands_by_function: HashMap::new(),
        diagnostic_owners: vec![None],
        diagnostics: Arc::new(vec![diagnostic]),
    }
}

fn semantic_use_table_for_body_input(
    module_id: ModuleId,
    values: &ValueResolution,
    locals: &LocalResolution,
    lowered: &TypeLowering,
    active_item_tree: &ActiveModuleItemTree,
) -> SemanticUseTable {
    let mut builder = SemanticUseTable::builder();
    for (key, local_use) in &locals.node_uses {
        match local_use {
            nia_local_resolve::LocalUse::Local(local_id) => {
                builder.insert_node_local_value_use(key.clone(), *local_id);
            }
            nia_local_resolve::LocalUse::Static(global_id) => {
                builder.insert_node_global_value_use(key.clone(), *global_id);
            }
            nia_local_resolve::LocalUse::ModuleValue
            | nia_local_resolve::LocalUse::Module
            | nia_local_resolve::LocalUse::TypePrefix
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
    builder
        .extend_node_type_uses(lowered.versioned_type_uses_from_active_item_tree(active_item_tree));
    builder.finish()
}

/// Checks bodies with program-wide signatures and caller-provided layouts.
pub fn check_module_bodies_with_program_signatures_and_layouts(
    input: BodyCheckInput<'_>,
) -> BodyCheck {
    check_module_bodies_with_program_signatures_and_layouts_with_timings(
        input,
        nia_timing::TimingMode::Off,
    )
}

/// Full body-check entry point with optional stage timing collection.
pub fn check_module_bodies_with_program_signatures_and_layouts_with_timings<'a>(
    input: BodyCheckInput<'a>,
    timings: nia_timing::TimingMode,
) -> BodyCheck {
    let timing = timings.detail();
    let module_id = input.defs.module_id;
    let prechecked = input.prechecked;
    let seed = input.seed;
    let visible_extensions = BodyVisibleExtensions {
        methods: input.extensions,
        lazy: input.lazy_extensions,
    };
    let extension_methods_by_id = time_body_stage(
        timing,
        "body_check.extension_method_lookup",
        module_id,
        || {
            BodyChecker::extension_method_lookup(
                module_id,
                input.defs,
                input.signatures,
                visible_extensions,
                input.normalization,
            )
        },
    );
    let extensions = if let Some(load) = input.lazy_extensions {
        BodyVisibleExtensionSource::Lazy {
            load,
            loaded: std::cell::OnceCell::new(),
        }
    } else {
        BodyVisibleExtensionSource::Eager(input.extensions.clone())
    };
    let types = BodyTypeCx::new(input.type_store, module_id);
    let unit_ty = types.intern(TyKind::Tuple(Vec::new()));
    let mut checker = time_body_stage(timing, "body_check.init", module_id, || BodyChecker {
        type_store: input.type_store,
        active_item_tree: input.active_item_tree,
        defs: input.defs,
        program: input.program,
        values: input.values,
        locals: input.locals,
        semantic_uses: input.semantic_uses,
        interner: types,
        type_lowering: input.lowered,
        signatures: input.signatures,
        const_signatures: input.const_signatures,
        normalization: input.normalization,
        target: input.target,
        const_eval: input.const_eval,
        const_module: input.const_module,
        layouts: input.layouts,
        extensions,
        program_extension_methods: input.program_extension_methods,
        program_signature_scope: match input.function_scope {
            FunctionCheckScope::LocalModule => ProgramSignatureScope::LocalModule,
            FunctionCheckScope::ProgramSignatures => {
                ProgramSignatureScope::Program(input.program_signatures.lookup)
            }
        },
        program_trait_impls: input.program_signatures.trait_impls,
        program_trait_impl_index: input.program_signatures.trait_impl_index,
        program_const_values: input.program_const.values,
        program_const_array_lengths: input.program_const.array_lengths,
        program_const_module: input.program_const.module,
        source_path: input.source_path,
        symbols: input.symbols,
        extension_methods_by_id,
        extension_method_lookup_cache: HashMap::new(),
        callable_extension_methods_by_name: SymbolMap::default(),
        provider_demands: Rc::new(RefCell::new(HashSet::new())),
        provider_demands_by_function: Rc::new(RefCell::new(HashMap::new())),
        node_expr_types: HashMap::new(),
        node_bracket_suffix_resolutions: HashMap::new(),
        node_pointer_array_to_slice_coercions: HashMap::new(),
        node_trait_object_coercions: HashMap::new(),
        node_trait_object_upcasts: HashMap::new(),
        node_builtin_values: HashMap::new(),
        node_associated_const_projections: HashMap::new(),
        node_array_repeat_counts: HashMap::new(),
        node_pattern_values: HashMap::new(),
        node_resolved_calls: HashMap::new(),
        node_function_references: HashMap::new(),
        inferred_closures: HashMap::new(),
        field_default_sources: HashMap::new(),
        field_default_templates: HashMap::new(),
        active_field_default_templates: HashSet::new(),
        current_field_default_owner: None,
        next_instantiated_local_id: u32::try_from(input.locals.locals.len())
            .unwrap_or(u32::MAX),
        generic_instantiations: Vec::new(),
        function_facts: HashMap::new(),
        function_bodies: HashMap::new(),
        global_inits: HashMap::new(),
        static_init_refs: HashMap::new(),
        local_types: HashMap::new(),
        global_types: HashMap::new(),
        const_types: HashMap::new(),
        method_receiver_kinds: HashMap::new(),
        traits_by_method_name: SymbolMap::default(),
        trait_impls_by_trait: HashMap::new(),
        def_trait_obligations_cache: HashMap::new(),
        trait_obligation_resolution_cache: HashMap::new(),
        type_match_cache: HashMap::new(),
        diagnostics: Vec::new(),
        diagnostic_owners: Vec::new(),
        timing,
        timing_module_id: module_id,
        current_return: unit_ty,
        current_def_id: None,
        next_closure_ordinal: 0,
        current_param_locals: Vec::new(),
        const_context_depth: 0,
        const_call_locals: Vec::new(),
        const_eval_budget: nia_const_eval::ConstEvalBudget::default(),
        body_filter: ActiveBodyCheckFilter::from_filter(input.filter),
        product: input.product,
        checked_functions: HashSet::new(),
        pending_functions: VecDeque::new(),
        profile: nia_timing::TimingAccumulator::default(),
    });
    if let Some(prechecked) = prechecked {
        time_body_stage(timing, "body_check.load_checked_facts", module_id, || {
            checker.load_checked_body_facts(module_id, prechecked);
        });
    } else {
        time_body_stage(timing, "body_check.seed_global_types", module_id, || {
            checker.seed_global_types();
            if let Some(seed) = seed {
                checker.load_type_facts(module_id, seed.facts);
            }
        });
        time_body_stage(timing, "body_check.check_module", module_id, || {
            checker.check_module(input.active_item_tree, timing, module_id);
        });
    }
    match checker.product {
        BodyCheckProduct::Full | BodyCheckProduct::BodyOnly => {
            checker.lower_field_default_templates();
            time_body_stage(timing, "body_check.lower_checked", module_id, || {
                checker.lower_checked_module(input.active_item_tree, timing, module_id);
            });
        }
        BodyCheckProduct::StaticInitOnly => {
            time_body_stage(timing, "body_check.lower_static_inits", module_id, || {
                checker.lower_checked_static_inits(input.active_item_tree);
            });
        }
        BodyCheckProduct::FactsOnly => {}
    }
    checker.print_profile();
    time_body_stage(timing, "body_check.finish", module_id, || {
        let mut facts = SemanticFactsBuilder {
            global_types: checker
                .global_types
                .into_iter()
                .map(|(def_id, ty)| (GlobalDefId { module_id, def_id }, ty))
                .collect(),
            const_types: checker
                .const_types
                .into_iter()
                .map(|(def_id, ty)| (GlobalDefId { module_id, def_id }, ty))
                .collect(),
            generic_instantiations: checker.generic_instantiations,
            function_facts: checker
                .function_facts
                .into_iter()
                .map(|(def_id, facts)| (def_id, facts.finish(input.semantic_uses.node_store())))
                .collect(),
            node_expr_types: checker.node_expr_types,
            node_bracket_suffix_resolutions: checker.node_bracket_suffix_resolutions,
            node_pointer_array_to_slice_coercions: checker.node_pointer_array_to_slice_coercions,
            node_trait_object_coercions: checker.node_trait_object_coercions,
            node_trait_object_upcasts: checker.node_trait_object_upcasts,
            node_builtin_values: checker.node_builtin_values,
            node_builtin_associated_values: input
                .semantic_uses
                .node_builtin_associated_values
                .iter()
                .map(|(key, value)| (key, *value))
                .collect(),
            node_associated_const_projections: checker.node_associated_const_projections,
            node_array_repeat_counts: checker.node_array_repeat_counts,
            node_pattern_values: checker.node_pattern_values,
            node_resolved_calls: checker.node_resolved_calls,
            node_function_references: checker.node_function_references,
        };
        facts.retain_module_level_facts();
        let facts = facts.finish(input.semantic_uses.node_store());
        checker
            .diagnostic_owners
            .resize(checker.diagnostics.len(), None);
        BodyCheck {
            ir: Arc::new(BodyIr {
                function_bodies: checker.function_bodies,
                global_inits: checker.global_inits,
            }),
            facts: Arc::new(facts),
            static_init_refs: checker.static_init_refs,
            checked_functions: checker.checked_functions,
            provider_demands: Arc::new(checker.provider_demands.borrow().clone()),
            provider_demands_by_function: checker.provider_demands_by_function.borrow().clone(),
            diagnostic_owners: checker.diagnostic_owners,
            diagnostics: Arc::new(checker.diagnostics),
        }
    })
}

pub(super) fn time_body_stage<T>(
    enabled: bool,
    name: &str,
    module_id: ModuleId,
    f: impl FnOnce() -> T,
) -> T {
    if !enabled {
        return f();
    }
    nia_timing::time_query(
        nia_timing::TimingMode::Detail,
        &format!("{name}[{module_id:?}]"),
        f,
    )
}

pub(super) fn time_body_stage_if_slow<T>(
    enabled: bool,
    name: &str,
    module_id: ModuleId,
    detail: impl fmt::Display,
    threshold_seconds: f64,
    f: impl FnOnce() -> T,
) -> T {
    if !enabled {
        return f();
    }
    nia_timing::time_query_if_slow(
        nia_timing::TimingMode::Detail,
        &format!("{name}[{module_id:?} {detail}]"),
        std::time::Duration::from_secs_f64(threshold_seconds),
        f,
    )
}

#[cfg(test)]
mod tests {
    use super::{body_check_target_layout_error, target_data_layout};
    use nia_target_config::TargetConfig;

    #[test]
    fn unsupported_target_pointer_widths_are_recoverable() {
        for pointer_width in [0, 129] {
            let target = TargetConfig {
                pointer_width,
                ..TargetConfig::host()
            };
            assert!(target_data_layout(&target).is_none());
            let check = body_check_target_layout_error(&target);
            assert!(check.ir.function_bodies.is_empty());
            assert!(check.ir.global_inits.is_empty());
            assert_eq!(check.diagnostics.len(), 1);
            assert_eq!(check.diagnostic_owners, vec![None]);
            assert!(check.diagnostics[0].summary.contains("pointer width"));
            assert!(
                check.diagnostics[0]
                    .summary
                    .contains(&pointer_width.to_string())
            );
        }

        let target = TargetConfig {
            pointer_width: 64,
            ..TargetConfig::host()
        };
        assert!(target_data_layout(&target).is_some());
    }
}
