// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExecutableFunctionBodyQuery(pub(super) GlobalDefId);

impl QueryKey<CompilerContext> for ExecutableFunctionBodyQuery {
    type Value = Option<Arc<nia_body_ir::TypedBody>>;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "executable_function_body"
    }

    fn description(&self) -> String {
        format!("executable_function_body({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        provide_executable_function_body(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

pub(super) fn materialize_executable_checked_modules(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<Vec<Arc<CheckedModule>>> {
    let facts = db.get(ExecutableCheckedModuleFactsQuery)?;
    let mut bodies_by_module =
        HashMap::<ModuleId, HashMap<GlobalDefId, Arc<nia_body_ir::TypedBody>>>::new();
    for def_id in facts.runtime_functions.iter().copied() {
        let body = match facts.function_bodies.get(&def_id) {
            Some(body) => Some(Arc::clone(body)),
            None => db
                .get(ExecutableFunctionBodyQuery(def_id))?
                .as_ref()
                .clone(),
        };
        if let Some(body) = body {
            bodies_by_module
                .entry(def_id.module_id)
                .or_default()
                .insert(def_id, body);
        }
    }
    let static_inits = facts
        .runtime_globals
        .iter()
        .copied()
        .map(|def_id| db.get(ExecutableStaticInitQuery(def_id)))
        .collect::<QueryResult<Vec<_>>>()?;
    let mut static_inits_by_module =
        HashMap::<ModuleId, HashMap<GlobalDefId, Arc<nia_static_ir::StaticInit>>>::new();
    for (def_id, init) in facts.runtime_globals.iter().copied().zip(static_inits) {
        if let Some(init) = init.as_ref() {
            static_inits_by_module
                .entry(def_id.module_id)
                .or_default()
                .insert(def_id, Arc::clone(init));
        }
    }

    Ok(facts
        .modules
        .iter()
        .map(|module| {
            let mut module = module.as_ref().clone();
            let function_bodies = bodies_by_module.remove(&module.id).unwrap_or_default();
            module.body_ir = Arc::new(nia_body_ir::BodyIr {
                function_bodies,
                global_inits: static_inits_by_module
                    .remove(&module.id)
                    .unwrap_or_default(),
            });
            Arc::new(module)
        })
        .collect())
}

pub(super) type LoweredFunctionBodyValue = nia_function_lower::LoweredFunctionBody;

#[derive(Debug, Clone)]
pub(super) struct LoweredFunctionBodyHandle {
    pub(super) def_id: GlobalDefId,
    pub(super) value: Arc<LoweredFunctionBodyValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct LoweredFunctionBodyQuery(pub(super) GlobalDefId);

impl QueryKey<CompilerContext> for LoweredFunctionBodyQuery {
    type Value = LoweredFunctionBodyValue;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "lowered_function_body"
    }

    fn description(&self) -> String {
        format!("lowered_function_body({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        provide_lowered_function_body(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}
