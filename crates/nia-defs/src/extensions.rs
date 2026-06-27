// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use crate::DefId;
use nia_ids::{GlobalDefId, InternedTyId, ModuleId, TraitId, TraitImplId, Visibility};
use nia_span::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WherePredicateSignature {
    pub ty: InternedTyId,
    pub bounds: Vec<WhereBoundSignature>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhereBoundSignature {
    pub trait_ty: InternedTyId,
    pub associated_type_bindings: Vec<AssociatedTypeBindingSignature>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssociatedTypeBindingSignature {
    pub name: String,
    pub ty: InternedTyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExtensionMethods {
    by_module: HashMap<ModuleId, Vec<ExtensionMethod>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionAssociatedValue {
    pub name: String,
    pub def_id: GlobalDefId,
    pub impl_id: TraitImplId,
    pub target_ty: InternedTyId,
    pub visibility: Visibility,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExtensionAssociatedValues {
    by_module: HashMap<ModuleId, Vec<ExtensionAssociatedValue>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleExtensionAssociatedValue {
    pub name: String,
    pub def_id: GlobalDefId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionMethod {
    pub name: String,
    pub def_id: GlobalDefId,
    pub impl_id: TraitImplId,
    pub impl_generics: Vec<String>,
    pub target_ty: InternedTyId,
    pub trait_id: Option<TraitId>,
    pub trait_args: Vec<InternedTyId>,
    pub where_predicates: Vec<WherePredicateSignature>,
    pub visibility: Visibility,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct VisibleExtensionMethods {
    targets: Vec<VisibleExtensionTarget>,
    callable_by_name: HashMap<String, Vec<(usize, usize)>>,
    trait_witnesses_by_name: HashMap<String, Vec<(usize, usize)>>,
    trait_witness_impls: HashSet<(ModuleId, TraitImplId)>,
    associated_values_by_target_name: HashMap<(InternedTyId, String), Vec<(usize, usize)>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleExtensionMethod {
    pub name: String,
    pub def_id: GlobalDefId,
    pub impl_id: TraitImplId,
    pub impl_generics: Vec<String>,
    pub trait_id: Option<TraitId>,
    pub trait_args: Vec<InternedTyId>,
    pub where_predicates: Vec<WherePredicateSignature>,
    pub is_callable: bool,
    pub is_trait_witness: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleExtensionTarget {
    pub impl_id: TraitImplId,
    pub target_ty: InternedTyId,
    pub methods: Vec<VisibleExtensionMethod>,
    pub associated_values: Vec<VisibleExtensionAssociatedValue>,
}

impl ExtensionMethods {
    pub fn insert(&mut self, module_id: ModuleId, method: ExtensionMethod) {
        self.by_module.entry(module_id).or_default().push(method);
    }

    pub fn visible_methods(
        &self,
        current_module: ModuleId,
        imported_modules: impl IntoIterator<Item = ModuleId>,
        visibility_allows: impl Fn(Visibility, ModuleId) -> bool,
    ) -> Vec<ExtensionMethod> {
        let mut methods = Vec::new();
        if let Some(module_methods) = self.by_module.get(&current_module) {
            methods.extend(module_methods.iter().cloned());
        }
        for module_id in imported_modules {
            if let Some(module_methods) = self.by_module.get(&module_id) {
                methods.extend(
                    module_methods
                        .iter()
                        .filter(|method| {
                            method.trait_id.is_some()
                                || visibility_allows(method.visibility, method.def_id.module_id)
                        })
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
            .flat_map(|methods| methods.iter())
            .map(|method| method.def_id.def_id)
            .collect()
    }

    pub fn all_methods(&self) -> impl Iterator<Item = &ExtensionMethod> {
        self.by_module.values().flat_map(|methods| methods.iter())
    }
}

impl ExtensionAssociatedValues {
    pub fn insert(&mut self, module_id: ModuleId, value: ExtensionAssociatedValue) {
        self.by_module.entry(module_id).or_default().push(value);
    }

    pub fn visible_values(
        &self,
        current_module: ModuleId,
        imported_modules: impl IntoIterator<Item = ModuleId>,
        visibility_allows: impl Fn(Visibility, ModuleId) -> bool,
    ) -> Vec<ExtensionAssociatedValue> {
        let mut values = Vec::new();
        if let Some(module_values) = self.by_module.get(&current_module) {
            values.extend(module_values.iter().cloned());
        }
        for module_id in imported_modules {
            if let Some(module_values) = self.by_module.get(&module_id) {
                values.extend(
                    module_values
                        .iter()
                        .filter(|value| visibility_allows(value.visibility, value.def_id.module_id))
                        .cloned(),
                );
            }
        }
        values
    }

    pub fn all_values(&self) -> impl Iterator<Item = &ExtensionAssociatedValue> {
        self.by_module.values().flat_map(|values| values.iter())
    }
}

impl VisibleExtensionMethods {
    pub fn insert(
        &mut self,
        impl_id: TraitImplId,
        target_ty: InternedTyId,
        method: VisibleExtensionMethod,
    ) {
        let target_index = self.target_index(impl_id, target_ty);
        let method_index = self.targets[target_index].methods.len();
        if method.is_callable {
            self.callable_by_name
                .entry(method.name.clone())
                .or_default()
                .push((target_index, method_index));
        }
        if method.is_trait_witness {
            self.trait_witnesses_by_name
                .entry(method.name.clone())
                .or_default()
                .push((target_index, method_index));
            self.trait_witness_impls
                .insert((method.def_id.module_id, method.impl_id));
        }
        self.targets[target_index].methods.push(method);
    }

    pub fn insert_trait_witness_impl(&mut self, module_id: ModuleId, impl_id: TraitImplId) {
        self.trait_witness_impls.insert((module_id, impl_id));
    }

    pub fn insert_associated_value(
        &mut self,
        impl_id: TraitImplId,
        target_ty: InternedTyId,
        value: VisibleExtensionAssociatedValue,
    ) {
        let target_index = self.target_index(impl_id, target_ty);
        let value_index = self.targets[target_index].associated_values.len();
        self.associated_values_by_target_name
            .entry((target_ty, value.name.clone()))
            .or_default()
            .push((target_index, value_index));
        self.targets[target_index].associated_values.push(value);
    }

    pub fn methods(&self, target_ty: InternedTyId, name: &str) -> Vec<GlobalDefId> {
        self.callable_by_name
            .get(name)
            .into_iter()
            .flat_map(|entries| entries.iter())
            .filter_map(|(target_index, method_index)| {
                let target = self.targets.get(*target_index)?;
                if target.target_ty != target_ty {
                    return None;
                }
                target
                    .methods
                    .get(*method_index)
                    .map(|method| method.def_id)
            })
            .collect()
    }

    pub fn all_methods_named(&self, name: &str) -> Vec<(InternedTyId, VisibleExtensionMethod)> {
        self.callable_by_name
            .get(name)
            .into_iter()
            .flat_map(|entries| entries.iter())
            .filter_map(|(target_index, method_index)| {
                let target = self.targets.get(*target_index)?;
                let method = target.methods.get(*method_index)?;
                Some((target.target_ty, method.clone()))
            })
            .collect()
    }

    pub fn all_trait_witnesses_named(
        &self,
        name: &str,
    ) -> Vec<(InternedTyId, VisibleExtensionMethod)> {
        self.trait_witnesses_by_name
            .get(name)
            .into_iter()
            .flat_map(|entries| entries.iter())
            .filter_map(|(target_index, method_index)| {
                let target = self.targets.get(*target_index)?;
                let method = target.methods.get(*method_index)?;
                Some((target.target_ty, method.clone()))
            })
            .collect()
    }

    pub fn targets(&self) -> &[VisibleExtensionTarget] {
        &self.targets
    }

    pub fn has_trait_witness_impl(&self, module_id: ModuleId, impl_id: TraitImplId) -> bool {
        self.trait_witness_impls.contains(&(module_id, impl_id))
    }

    pub fn trait_witness_impls(&self) -> impl Iterator<Item = (ModuleId, TraitImplId)> + '_ {
        self.trait_witness_impls.iter().copied()
    }

    pub fn associated_value(
        &self,
        target_ty: InternedTyId,
        name: &str,
    ) -> Option<VisibleExtensionAssociatedValue> {
        let mut matches = self
            .associated_values_by_target_name
            .get(&(target_ty, name.to_string()))?
            .iter()
            .filter_map(|(target_index, value_index)| {
                self.targets
                    .get(*target_index)?
                    .associated_values
                    .get(*value_index)
            });
        let first = matches.next()?.clone();
        if matches.next().is_some() {
            return None;
        }
        Some(first)
    }

    fn target_index(&mut self, impl_id: TraitImplId, target_ty: InternedTyId) -> usize {
        if let Some(index) = self
            .targets
            .iter()
            .position(|item| item.impl_id == impl_id && item.target_ty == target_ty)
        {
            return index;
        }
        let index = self.targets.len();
        self.targets.push(VisibleExtensionTarget {
            impl_id,
            target_ty,
            methods: Vec::new(),
            associated_values: Vec::new(),
        });
        index
    }
}
