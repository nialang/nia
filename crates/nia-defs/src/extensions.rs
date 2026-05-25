// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::DefId;
use nia_ast::Visibility;
use nia_ids::{GlobalDefId, ModuleId, TyId};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExtensionMethods {
    by_module: HashMap<ModuleId, HashMap<GlobalDefId, Vec<ExtensionMethod>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionMethod {
    pub def_id: GlobalDefId,
    pub target_args: Vec<TyId>,
    pub visibility: Visibility,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct VisibleExtensionMethods {
    targets: Vec<VisibleExtensionTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleExtensionMethod {
    pub name: String,
    pub def_id: GlobalDefId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleExtensionTarget {
    pub target: GlobalDefId,
    pub args: Vec<TyId>,
    pub methods: Vec<VisibleExtensionMethod>,
}

impl ExtensionMethods {
    pub fn insert(&mut self, module_id: ModuleId, target: GlobalDefId, method: ExtensionMethod) {
        self.by_module
            .entry(module_id)
            .or_default()
            .entry(target)
            .or_default()
            .push(method);
    }

    pub fn visible_methods(
        &self,
        current_module: ModuleId,
        imported_modules: impl IntoIterator<Item = ModuleId>,
        target: GlobalDefId,
    ) -> Vec<ExtensionMethod> {
        let mut methods = Vec::new();
        if let Some(module_methods) = self.by_module.get(&current_module)
            && let Some(target_methods) = module_methods.get(&target)
        {
            methods.extend(target_methods.iter().cloned());
        }
        for module_id in imported_modules {
            if let Some(module_methods) = self.by_module.get(&module_id)
                && let Some(target_methods) = module_methods.get(&target)
            {
                methods.extend(
                    target_methods
                        .iter()
                        .filter(|method| method.visibility == Visibility::Public)
                        .cloned(),
                );
            }
        }
        methods
    }

    pub fn method_ids_for_module(&self, module_id: ModuleId) -> Vec<DefId> {
        self.by_module
            .get(&module_id)
            .into_iter()
            .flat_map(|targets| targets.values())
            .flat_map(|methods| methods.iter())
            .map(|method| method.def_id.def_id)
            .collect()
    }
}

impl VisibleExtensionMethods {
    pub fn insert(&mut self, target: GlobalDefId, args: Vec<TyId>, method: VisibleExtensionMethod) {
        if let Some(existing) = self
            .targets
            .iter_mut()
            .find(|item| item.target == target && item.args == args)
        {
            existing.methods.push(method);
            return;
        }
        self.targets.push(VisibleExtensionTarget {
            target,
            args,
            methods: vec![method],
        });
    }

    pub fn methods(&self, target: GlobalDefId, name: &str) -> Vec<GlobalDefId> {
        self.targets
            .iter()
            .filter(|item| item.target == target)
            .flat_map(|item| item.methods.iter())
            .filter(|method| method.name == name)
            .map(|method| method.def_id)
            .collect()
    }

    pub fn targets(&self) -> &[VisibleExtensionTarget] {
        &self.targets
    }
}
