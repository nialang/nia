// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) struct TestBackendLowering {
    pub(super) lowering: BackendLowering,
    pub(super) module_id: ModuleId,
    pub(super) type_store: nia_ty::TypeStore,
}

pub(super) fn backend_function_instance_plan(
    monomorphization: &nia_monomorphize::Monomorphization,
) -> Vec<BackendFunctionInstancePlan> {
    monomorphization
        .instances
        .iter()
        .map(|instance| BackendFunctionInstancePlan {
            def_id: instance.def_id,
            arg_module_id: instance.arg_module_id,
            self_arg: instance.self_arg,
            args: instance.args.clone(),
            const_args: instance.const_args.clone(),
            span: instance.span,
        })
        .collect()
}

impl std::ops::Deref for TestBackendLowering {
    type Target = BackendLowering;

    fn deref(&self) -> &Self::Target {
        &self.lowering
    }
}

impl TestBackendLowering {
    pub(super) fn append(&self, module_id: ModuleId) -> nia_ty::TypeStoreAppend {
        self.type_store.append_for_module(module_id)
    }
}

pub(super) fn local_name(text: &str) -> nia_function_ir::LocalName {
    nia_function_ir::LocalName::named(sym(text))
}

pub(super) fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(nia_symbol::stable_hash(text))
}
