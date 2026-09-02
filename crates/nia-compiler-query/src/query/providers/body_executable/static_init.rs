// SPDX-License-Identifier: GPL-3.0-or-later
//! On-demand reconstruction of executable static initializers.

use super::*;

pub(in crate::query) fn provide_executable_static_init(
    db: &QueryDb<CompilerContext>,
    def_id: GlobalDefId,
) -> QueryResult<Option<Arc<nia_static_ir::StaticInit>>> {
    let facts = db.get(ExecutableCheckedModuleFactsQuery)?;
    if facts.runtime_globals.binary_search(&def_id).is_err() {
        return Ok(None);
    }
    let Some(module) = facts
        .modules
        .iter()
        .find(|module| module.id == def_id.module_id)
    else {
        return Ok(None);
    };
    let semantic_facts = executable_static_init_semantic_facts(module, def_id);
    let functions = HashSet::new();
    let globals = HashSet::from([def_id]);
    let resolution_inputs = BodyCheckResolutionInputs {
        active_item_tree: db.get(FullActiveModuleItemTreeQuery(def_id.module_id))?,
        values: Arc::clone(&module.value_resolution),
        locals: Arc::clone(&module.local_resolution),
        semantic_uses: Arc::clone(&module.semantic_uses),
        resolution_diagnostics: module.resolution_diagnostics.clone(),
    };
    let checked = body_check_with_filter_and_layouts_with_inputs(
        db,
        ExecutableBodyCheckInput {
            module_id: def_id.module_id,
            filter: nia_body_check::BodyCheckFilter::ReachableItems {
                functions: &functions,
                globals: &globals,
                already_checked_functions: None,
                already_checked_globals: Some(&globals),
            },
            layouts: Some(Arc::clone(&module.layouts)),
            program_layouts_override: None,
            fact_mode: ExecutableFactMode::executable(ReachableBodyModules::new(
                &facts.reachable_body_modules,
            )),
            resolution_inputs: Some(resolution_inputs),
            seed: None,
            global_initializer_cache: None,
            const_module_cache: None,
            const_inputs: Some((
                &module.const_eval,
                facts
                    .const_modules
                    .get(&def_id.module_id)
                    .expect("executable static module must retain const lowering")
                    .as_ref(),
            )),
            program_function_signature_cache: None,
            product: nia_body_check::BodyCheckProduct::StaticInitOnly,
            prechecked: Some(nia_body_check::PrecheckedBodyCheck {
                ir: nia_body_ir::BodyIr {
                    function_bodies: HashMap::new(),
                    global_inits: HashMap::new(),
                },
                facts: semantic_facts,
                checked_functions: HashSet::new(),
                diagnostic_owners: Vec::new(),
                diagnostics: Vec::new(),
            }),
        },
    )?;
    Ok(checked.body_check.ir.global_inits.get(&def_id).cloned())
}

fn executable_static_init_semantic_facts(
    module: &CheckedModule,
    def_id: GlobalDefId,
) -> nia_sema_ir::SemanticFacts {
    let node_store = module.semantic_facts.node_store().clone();
    let owner = module
        .defs
        .defs
        .get(def_id.def_id)
        .and_then(|def| def.parent)
        .map(|def_id| GlobalDefId {
            module_id: module.id,
            def_id,
        });
    let mut facts = module.semantic_facts.as_ref().clone().into_builder();
    let owner_facts = owner
        .and_then(|owner| facts.function_facts.remove(&owner))
        .map(nia_sema_ir::FunctionSemanticFacts::into_builder);
    facts.function_facts.clear();
    // A local static initializer is stored under its enclosing function, but
    // StaticInitOnly checks consume module-level node maps. Flatten only that
    // owner's facts so unrelated function bodies stay outside this query.
    if let Some(owner_facts) = owner_facts {
        facts.node_expr_types.extend(owner_facts.node_expr_types);
        facts
            .node_bracket_suffix_resolutions
            .extend(owner_facts.node_bracket_suffix_resolutions);
        facts
            .node_pointer_array_to_slice_coercions
            .extend(owner_facts.node_pointer_array_to_slice_coercions);
        facts
            .node_trait_object_coercions
            .extend(owner_facts.node_trait_object_coercions);
        facts
            .node_trait_object_upcasts
            .extend(owner_facts.node_trait_object_upcasts);
        facts
            .node_builtin_values
            .extend(owner_facts.node_builtin_values);
        facts
            .node_associated_const_projections
            .extend(owner_facts.node_associated_const_projections);
        facts
            .node_array_repeat_counts
            .extend(owner_facts.node_array_repeat_counts);
        facts
            .node_pattern_values
            .extend(owner_facts.node_pattern_values);
        facts
            .node_resolved_calls
            .extend(owner_facts.node_resolved_calls);
        facts
            .node_function_references
            .extend(owner_facts.node_function_references);
    }
    facts.finish(&node_store)
}
