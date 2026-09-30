// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

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
    let target_layout = nia_layout::TargetDataLayout::for_target(*input.target);
    let layouts =
        match nia_layout::compute_layouts_with_program_context(nia_layout::LayoutComputationInput {
            type_store: input.type_store,
            defs: input.defs,
            signatures: input.signatures,
            root_types: &root_types,
            normalized: &input.normalization.normalized,
            array_lengths: &array_lengths,
            target: target_layout,
            program: nia_layout::ProgramLayoutContext::default(),
        }) {
            Ok(layouts) => layouts,
            Err(error) => return body_check_internal_error(error),
        };
    let mut checked = check_module_bodies_with_layouts(BodyCheckInput {
        type_store: input.type_store,
        source_version: input.source_version,
        source_path: input.source_path,
        source_text: input.source_text,
        symbols: input.symbols,
        origins: input.origins,
        active_item_tree: input.active_item_tree,
        defs: input.defs,
        values: input.values,
        locals: input.locals,
        semantic_uses: input.semantic_uses,
        type_resolution: input.type_resolution,
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

fn body_check_internal_error(error: nia_ice::Ice) -> BodyCheck {
    BodyCheck {
        internal_error: Some(error),
        ir: Arc::new(BodyIr {
            function_bodies: HashMap::new(),
            global_inits: HashMap::new(),
        }),
        facts: Arc::new(SemanticFacts::default()),
        static_init_refs: HashMap::new(),
        checked_functions: HashSet::new(),
        provider_demands: Arc::new(HashSet::new()),
        provider_demands_by_function: HashMap::new(),
        diagnostic_owners: Vec::new(),
        diagnostics: Arc::new(Vec::new()),
    }
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
        BodyVisibleExtensionSource::Eager(input.extensions)
    };
    let failure = BodyFailure::new();
    let types = BodyTypeCx::new(input.type_store, module_id, failure.clone());
    let unit_ty = types.intern(TyKind::Tuple(Vec::new()));
    if let Some(error) = failure.internal_error() {
        return body_check_internal_error(error);
    }
    let mut checker = time_body_stage(timing, "body_check.init", module_id, || BodyChecker {
        failure,
        type_store: input.type_store,
        active_item_tree: input.active_item_tree,
        defs: input.defs,
        program: input.program,
        values: input.values,
        locals: input.locals,
        semantic_uses: input.semantic_uses,
        type_resolution: input.type_resolution,
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
        source_text: input.source_text,
        symbols: input.symbols,
        extension_methods_by_id,
        extension_method_lookup_cache: FastHashMap::default(),
        callable_extension_methods_by_name: SymbolMap::default(),
        provider_demands: Rc::new(RefCell::new(HashSet::new())),
        provider_demands_by_function: Rc::new(RefCell::new(HashMap::new())),
        node_expr_types: FastHashMap::default(),
        node_bracket_suffix_resolutions: FastHashMap::default(),
        node_pointer_array_to_slice_coercions: FastHashMap::default(),
        node_function_pointer_to_callable_coercions: FastHashMap::default(),
        node_trait_object_coercions: FastHashMap::default(),
        node_trait_object_upcasts: FastHashMap::default(),
        node_builtin_values: FastHashMap::default(),
        node_associated_const_projections: FastHashMap::default(),
        node_array_repeat_counts: FastHashMap::default(),
        node_pattern_values: FastHashMap::default(),
        node_resolved_calls: FastHashMap::default(),
        node_function_references: FastHashMap::default(),
        inferred_closures: FastHashMap::default(),
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
        trait_impls_by_trait: FastHashMap::default(),
        def_trait_obligations_cache: FastHashMap::default(),
        trait_obligation_resolution_cache: FastHashMap::default(),
        type_match_cache: FastHashMap::default(),
        diagnostics: Vec::new(),
        diagnostic_owners: Vec::new(),
        timing,
        timing_module_id: module_id,
        current_return: unit_ty,
        current_def_id: None,
        current_naked: false,
        closure_ordinals: FastHashMap::default(),
        current_param_locals: Vec::new(),
        const_context_depth: 0,
        const_call_locals: Vec::new(),
        const_eval_budget: nia_const_eval::ConstEvalBudget::default(),
        body_filter: ActiveBodyCheckFilter::from_filter(input.filter),
        flow_summary: None,
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
    if let Some(error) = checker.failure.internal_error() {
        checker.print_profile();
        return body_check_internal_error(error);
    }
    match checker.product {
        BodyCheckProduct::Full | BodyCheckProduct::BodyOnly => {
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
    if let Some(error) = checker.failure.internal_error() {
        return body_check_internal_error(error);
    }
    time_body_stage(timing, "body_check.finish", module_id, || {
        let facts = SemanticFactsBuilder {
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
            node_function_pointer_to_callable_coercions: checker
                .node_function_pointer_to_callable_coercions,
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
        let facts = facts.finish(input.semantic_uses.node_store());
        checker
            .diagnostic_owners
            .resize(checker.diagnostics.len(), None);
        BodyCheck {
            internal_error: None,
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
    use super::{SemanticFacts, body_check_internal_error};
    use nia_ice::Ice;

    #[test]
    fn internal_failure_does_not_publish_partial_products() {
        let check = body_check_internal_error(Ice::new("body-check invariant failed"));

        assert_eq!(
            check
                .internal_error
                .as_ref()
                .map(|error| error.message.as_str()),
            Some("body-check invariant failed")
        );
        assert!(check.ir.function_bodies.is_empty());
        assert!(check.ir.global_inits.is_empty());
        assert_eq!(*check.facts, SemanticFacts::default());
        assert!(check.static_init_refs.is_empty());
        assert!(check.checked_functions.is_empty());
        assert!(check.provider_demands.is_empty());
        assert!(check.provider_demands_by_function.is_empty());
        assert!(check.diagnostic_owners.is_empty());
        assert!(check.diagnostics.is_empty());
    }
}
