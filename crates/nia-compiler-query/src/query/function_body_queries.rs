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

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        provide_executable_function_body(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

pub(super) fn materialize_executable_checked_modules(
    db: &QueryDb<CompilerContext>,
) -> Vec<Arc<CheckedModule>> {
    let facts = db.get(ExecutableCheckedModuleFactsQuery);
    let bodies = db.get_many(
        facts
            .runtime_functions
            .iter()
            .copied()
            .map(ExecutableFunctionBodyQuery),
    );
    let static_inits = db.get_many(
        facts
            .runtime_globals
            .iter()
            .copied()
            .map(ExecutableStaticInitQuery),
    );
    let mut bodies_by_module =
        HashMap::<ModuleId, HashMap<GlobalDefId, Arc<nia_body_ir::TypedBody>>>::new();
    for (def_id, body) in facts.runtime_functions.iter().copied().zip(bodies) {
        if let Some(body) = body.as_ref() {
            bodies_by_module
                .entry(def_id.module_id)
                .or_default()
                .insert(def_id, Arc::clone(body));
        }
    }
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

    facts
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
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum LoweredFunctionBodyValue {
    Body(nia_function_ir::FunctionBody),
    Diagnostic(nia_function_lower::FunctionLoweringDiagnostic),
}

impl LoweredFunctionBodyValue {
    pub(super) fn body(&self) -> Option<&nia_function_ir::FunctionBody> {
        match self {
            Self::Body(body) => Some(body),
            Self::Diagnostic(_) => None,
        }
    }

    pub(super) fn diagnostic(&self) -> Option<&nia_function_lower::FunctionLoweringDiagnostic> {
        match self {
            Self::Body(_) => None,
            Self::Diagnostic(diagnostic) => Some(diagnostic),
        }
    }
}

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

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        provide_lowered_function_body(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}
