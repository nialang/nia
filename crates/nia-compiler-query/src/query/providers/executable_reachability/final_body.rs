// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

// Provider settlement can publish intermediate products between two final
// checks. Compare the values observed at finalization, including diagnostics.
macro_rules! final_input {
    ($name:ident, $source:ident, $label:literal $(, $module:ident)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub(in crate::query) struct $name $( (pub(in crate::query) $module) )?;
        impl QueryKey<CompilerContext> for $name {
            type Value = Arc<<$source as QueryKey<CompilerContext>>::Value>;
            const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;
            fn name() -> &'static str { $label }
            fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
                db.get($source $(({ let id: $module = self.0; id }))?)
            }
            fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {old == new}
        }
    };
}
final_input!(
    FinalVisibleTraitImplsQuery,
    VisibleTraitImplsQuery,
    "final_visible_trait_impls",
    ModuleId
);
final_input!(
    FinalConstModuleQuery,
    ConstModuleQuery,
    "final_const_module",
    ModuleId
);
final_input!(
    FinalConstValuesQuery,
    ConstValuesQuery,
    "final_const_values",
    ModuleId
);
final_input!(
    FinalTraitMethodIndexQuery,
    ProgramTraitMethodIndexQuery,
    "final_trait_method_index"
);

pub(in crate::query) struct ExecutableReachabilityPlan {
    pub(super) reachable: nia_executable_reachability::ExecutableReachability,
    pub(super) selected: HashMap<ModuleId, ExecutableFinalSelection>,
    pub(super) checks: HashMap<ModuleId, Arc<nia_body_check::PrecheckedBodyCheck>>,
    pub(super) demands: HashMap<ModuleId, HashSet<crate::ProviderDemand>>,
    pub(super) checked: Vec<ModuleId>,
    pub(super) type_only: Vec<ModuleId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::query) struct ExecutableReachabilityPlanQuery;
impl QueryKey<CompilerContext> for ExecutableReachabilityPlanQuery {
    type Value = ExecutableReachabilityPlan;
    fn name() -> &'static str {
        "executable_reachability_plan"
    }
    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        match executable_check(db, ExecutableCheckProduct::Reachability)? {
            ExecutableCheckOutput::Plan(plan) => Ok(plan),
            _ => Err(QueryError::internal(
                "reachability returned finalized modules",
            )),
        }
    }
}

#[derive(Default, Clone, PartialEq)]
pub(in crate::query) struct ExecutableFinalSelection {
    pub(super) checked: bool,
    pub(super) functions: HashSet<GlobalDefId>,
    pub(super) globals: HashSet<GlobalDefId>,
    pub(super) body: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::query) struct ExecutableFinalSelectionQuery(pub(in crate::query) ModuleId);

impl QueryKey<CompilerContext> for ExecutableFinalSelectionQuery {
    type Value = ExecutableFinalSelection;
    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;
    fn name() -> &'static str {
        "executable_final_selection"
    }
    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        Ok(db
            .get(ExecutableReachabilityPlanQuery)?
            .selected
            .get(&self.0)
            .cloned()
            .unwrap_or_default())
    }
    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::query) struct ExecutablePrecheckedModuleQuery(pub(in crate::query) ModuleId);

impl QueryKey<CompilerContext> for ExecutablePrecheckedModuleQuery {
    type Value = Option<Arc<nia_body_check::PrecheckedBodyCheck>>;
    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;
    fn name() -> &'static str {
        "executable_prechecked_module"
    }
    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        Ok(db
            .get(ExecutableReachabilityPlanQuery)?
            .checks
            .get(&self.0)
            .cloned())
    }
    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::query) struct ExecutableFinalLayoutQuery(pub(in crate::query) ModuleId);

impl QueryKey<CompilerContext> for ExecutableFinalLayoutQuery {
    type Value = ModuleLayouts;
    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;
    fn name() -> &'static str {
        "executable_final_layout"
    }
    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        let selection = db.get(ExecutableFinalSelectionQuery(self.0))?;
        let failure = RefCell::new(None);
        let lookup = |module| {
            capture_query_failure(&failure, db.get(ExecutableFinalSelectionQuery(module)))
                .is_some_and(|selection| selection.body)
        };
        let layouts = executable_layouts_for_reachable_items(
            db,
            self.0,
            &selection.functions,
            &selection.globals,
            ExecutableLayoutCaches {
                array_lengths: None,
                fact_layouts: None,
            },
            None,
            Some(ReachableBodyModules::tracked(&lookup)),
        )?;
        match failure.into_inner() {
            Some(error) => Err(error),
            None => Ok(layouts),
        }
    }
    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::query) struct ExecutableFinalBodyQuery(pub(in crate::query) ModuleId);

impl QueryKey<CompilerContext> for ExecutableFinalBodyQuery {
    type Value = (CheckedModule, Arc<nia_const_ir::ResolvedConstModule>);
    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;
    fn name() -> &'static str {
        "executable_final_body"
    }
    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        let id = self.0;
        let prechecked = db.get(ExecutablePrecheckedModuleQuery(id))?;
        let selection = db.get(ExecutableFinalSelectionQuery(id))?;
        let layouts = db.get(ExecutableFinalLayoutQuery(id))?;
        let failure = RefCell::new(None);
        let lookup = |module| {
            if capture_query_failure(&failure, db.get(ExecutableFinalSelectionQuery(module)))?
                .checked
            {
                capture_query_failure(&failure, db.get(ExecutableFinalLayoutQuery(module)))
                    .map(|layouts| layouts.semantic.clone())
            } else {
                capture_query_failure(&failure, signature_layouts_for_types(db, module, None))
                    .map(Arc::new)
            }
        };
        let body_selected = |module| {
            capture_query_failure(&failure, db.get(ExecutableFinalSelectionQuery(module)))
                .is_some_and(|selection| selection.body)
        };
        let inputs = db.get(FullBodyResolutionInputsQuery(id))?;
        let body = time_module_provider(db, "executable_body_check", id, || {
            body_check_with_filter_and_layouts_with_inputs(
                db,
                ExecutableBodyCheckInput {
                    module_id: id,
                    filter: nia_body_check::BodyCheckFilter::ReachableItems {
                        functions: &selection.functions,
                        globals: &selection.globals,
                        already_checked_functions: None,
                        already_checked_globals: None,
                    },
                    layouts: Some(Arc::clone(&layouts.semantic)),
                    program_layouts_override: Some(&lookup),
                    fact_mode: ExecutableFactMode::finalizing(ReachableBodyModules::tracked(
                        &body_selected,
                    )),
                    resolution_inputs: Some(inputs.as_ref().clone()),
                    seed: None,
                    global_initializer_cache: None,
                    const_module_cache: None,
                    const_inputs: None,
                    program_function_signature_cache: None,
                    product: nia_body_check::BodyCheckProduct::Full,
                    prechecked: prechecked.as_deref().cloned(),
                },
            )
        })?;
        if let Some(error) = failure.into_inner() {
            return Err(error);
        }
        let flow = executable_flow_check(db, id, &body.body_check.checked_functions)?;
        let lowering = body.const_module.clone().ok_or_else(|| {
            QueryError::internal(format!(
                "executable module {id:?} did not produce const lowering"
            ))
        })?;
        let module = executable_checked_module_with_body_and_flow_check(
            db,
            id,
            body,
            flow,
            layouts.as_ref().clone(),
        )?;
        Ok((module, lowering))
    }
    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}
