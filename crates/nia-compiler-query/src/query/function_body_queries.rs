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
        db.get(ExecutableCheckedModulesQuery)
            .iter()
            .find(|module| module.id == self.0.module_id)
            .and_then(|module| module.body_ir.function_bodies.get(&self.0))
            .cloned()
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
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
