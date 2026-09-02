// SPDX-License-Identifier: GPL-3.0-or-later
//! On-demand reconstruction of executable function bodies.

use super::*;

pub(in crate::query) fn provide_executable_function_body(
    db: &QueryDb<CompilerContext>,
    def_id: GlobalDefId,
) -> QueryResult<Option<Arc<nia_body_ir::TypedBody>>> {
    let facts = db.get(ExecutableCheckedModuleFactsQuery)?;
    if facts.runtime_functions.binary_search(&def_id).is_err() {
        return Ok(None);
    }
    let Some(module) = facts
        .modules
        .iter()
        .find(|module| module.id == def_id.module_id)
    else {
        return Ok(None);
    };
    // Rechecking one body must not retain facts owned by sibling functions.
    // Module-level type/const facts remain available because the selected body
    // may reference them without owning them.
    let function_facts = module
        .semantic_facts
        .function_facts
        .get(&def_id)
        .cloned()
        .map(|facts| HashMap::from([(def_id, facts)]))
        .unwrap_or_default();
    let semantic_facts = nia_sema_ir::SemanticFactsBuilder {
        global_types: module.semantic_facts.global_types.clone(),
        const_types: module.semantic_facts.const_types.clone(),
        function_facts,
        ..nia_sema_ir::SemanticFactsBuilder::default()
    }
    .finish(module.semantic_facts.node_store());
    let functions = HashSet::from([def_id]);
    let globals = HashSet::new();
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
                already_checked_globals: None,
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
                    .expect("executable function module must retain const lowering")
                    .as_ref(),
            )),
            program_function_signature_cache: None,
            product: nia_body_check::BodyCheckProduct::BodyOnly,
            prechecked: Some(nia_body_check::PrecheckedBodyCheck {
                ir: nia_body_ir::BodyIr {
                    function_bodies: HashMap::new(),
                    global_inits: HashMap::new(),
                },
                facts: semantic_facts,
                checked_functions: functions.clone(),
                diagnostic_owners: Vec::new(),
                diagnostics: Vec::new(),
                field_default_templates: HashMap::new(),
            }),
        },
    )?;
    Ok(checked.body_check.ir.function_bodies.get(&def_id).cloned())
}
