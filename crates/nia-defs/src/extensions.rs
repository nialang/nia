// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use crate::DefId;
use nia_ids::{GlobalDefId, InternedTyId, ModuleId, TraitId, TraitImplId, Visibility};
use nia_span::Span;
use nia_symbol::{SymbolId, SymbolMap};

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
    pub name: SymbolId,
    pub ty: InternedTyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExtensionMethods {
    by_module: HashMap<ModuleId, Vec<GlobalDefId>>,
    by_nominal_target: HashMap<GlobalDefId, Vec<GlobalDefId>>,
    by_name: SymbolMap<Vec<GlobalDefId>>,
    by_id: HashMap<GlobalDefId, ExtensionMethod>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionAssociatedValue {
    pub name: SymbolId,
    pub def_id: GlobalDefId,
    pub impl_id: TraitImplId,
    pub target_ty: InternedTyId,
    pub trait_id: Option<TraitId>,
    pub visibility: Visibility,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExtensionAssociatedValues {
    by_module: HashMap<ModuleId, Vec<GlobalDefId>>,
    by_nominal_target: HashMap<GlobalDefId, Vec<GlobalDefId>>,
    by_id: HashMap<GlobalDefId, ExtensionAssociatedValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleExtensionAssociatedValue {
    pub name: SymbolId,
    pub def_id: GlobalDefId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionMethod {
    pub name: SymbolId,
    pub def_id: GlobalDefId,
    pub impl_id: TraitImplId,
    pub effective_generics: Vec<SymbolId>,
    pub effective_const_generics: Vec<SymbolId>,
    pub target_ty: InternedTyId,
    pub trait_id: Option<TraitId>,
    pub trait_args: Vec<InternedTyId>,
    /// Const arguments of the implemented trait instance.
    pub trait_const_args: Vec<nia_ty::ConstGenericArg>,
    pub where_predicates: Vec<WherePredicateSignature>,
    pub visibility: Visibility,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct VisibleExtensionMethods {
    targets: Vec<VisibleExtensionTarget>,
    callable_by_name: SymbolMap<Vec<(usize, usize)>>,
    trait_witnesses_by_name: SymbolMap<Vec<(usize, usize)>>,
    trait_witness_impls: HashSet<(ModuleId, TraitImplId)>,
    associated_values_by_target_name: HashMap<(InternedTyId, SymbolId), Vec<(usize, usize)>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleExtensionMethod {
    pub name: SymbolId,
    pub def_id: GlobalDefId,
    pub impl_id: TraitImplId,
    pub effective_generics: Vec<SymbolId>,
    pub effective_const_generics: Vec<SymbolId>,
    pub trait_id: Option<TraitId>,
    pub trait_args: Vec<InternedTyId>,
    /// Const arguments of the implemented trait instance.
    pub trait_const_args: Vec<nia_ty::ConstGenericArg>,
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
    pub fn extend(&mut self, other: Self) {
        for (module_id, method_ids) in other.by_module {
            self.by_module
                .entry(module_id)
                .or_default()
                .extend(method_ids);
        }
        for (target, method_ids) in other.by_nominal_target {
            self.by_nominal_target
                .entry(target)
                .or_default()
                .extend(method_ids);
        }
        for (name, method_ids) in other.by_name {
            self.by_name.entry(name).or_default().extend(method_ids);
        }
        self.by_id.extend(other.by_id);
    }

    pub fn insert(&mut self, module_id: ModuleId, method: ExtensionMethod) {
        let def_id = method.def_id;
        self.by_name.entry(method.name).or_default().push(def_id);
        self.by_module.entry(module_id).or_default().push(def_id);
        self.by_id.insert(def_id, method);
    }

    pub fn insert_with_nominal_target(
        &mut self,
        module_id: ModuleId,
        method: ExtensionMethod,
        nominal_target: Option<GlobalDefId>,
    ) {
        let def_id = method.def_id;
        self.insert(module_id, method);
        if let Some(nominal_target) = nominal_target {
            self.by_nominal_target
                .entry(nominal_target)
                .or_default()
                .push(def_id);
        }
    }

    pub fn for_each_visible_method(
        &self,
        current_module: ModuleId,
        imported_modules: impl IntoIterator<Item = ModuleId>,
        visibility_allows: impl Fn(Visibility, ModuleId) -> bool,
        mut f: impl FnMut(&ExtensionMethod),
    ) {
        if let Some(method_ids) = self.by_module.get(&current_module) {
            for def_id in method_ids {
                if let Some(method) = self.by_id.get(def_id) {
                    f(method);
                }
            }
        }
        for module_id in imported_modules {
            if let Some(method_ids) = self.by_module.get(&module_id) {
                for def_id in method_ids {
                    let Some(method) = self.by_id.get(def_id) else {
                        continue;
                    };
                    if method.trait_id.is_some()
                        || visibility_allows(method.visibility, method.def_id.module_id)
                    {
                        f(method);
                    }
                }
            }
        }
    }

    pub fn visible_methods(
        &self,
        current_module: ModuleId,
        imported_modules: impl IntoIterator<Item = ModuleId>,
        visibility_allows: impl Fn(Visibility, ModuleId) -> bool,
    ) -> Vec<ExtensionMethod> {
        let mut methods = Vec::new();
        self.for_each_visible_method(
            current_module,
            imported_modules,
            visibility_allows,
            |method| methods.push(method.clone()),
        );
        methods
    }

    pub fn method_ids_for_module(&self, module_id: ModuleId) -> Vec<DefId> {
        self.by_module
            .get(&module_id)
            .into_iter()
            .flat_map(|method_ids| method_ids.iter())
            .map(|def_id| def_id.def_id)
            .collect()
    }

    pub fn all_methods(&self) -> impl Iterator<Item = &ExtensionMethod> {
        self.by_id.values()
    }

    pub fn methods_named(&self, name: &SymbolId) -> impl Iterator<Item = &ExtensionMethod> {
        self.by_name
            .get(name)
            .into_iter()
            .flat_map(|method_ids| method_ids.iter())
            .filter_map(|def_id| self.by_id.get(def_id))
    }

    pub fn method_by_id(&self, def_id: GlobalDefId) -> Option<&ExtensionMethod> {
        self.by_id.get(&def_id)
    }

    pub fn methods_for_nominal_target(
        &self,
        target_def_id: GlobalDefId,
    ) -> impl Iterator<Item = &ExtensionMethod> {
        self.by_nominal_target
            .get(&target_def_id)
            .into_iter()
            .flat_map(|method_ids| method_ids.iter())
            .filter_map(|def_id| self.by_id.get(def_id))
    }
}

impl ExtensionAssociatedValues {
    pub fn extend(&mut self, other: Self) {
        for (module_id, value_ids) in other.by_module {
            self.by_module
                .entry(module_id)
                .or_default()
                .extend(value_ids);
        }
        for (target, value_ids) in other.by_nominal_target {
            self.by_nominal_target
                .entry(target)
                .or_default()
                .extend(value_ids);
        }
        self.by_id.extend(other.by_id);
    }

    pub fn insert(&mut self, module_id: ModuleId, value: ExtensionAssociatedValue) {
        let def_id = value.def_id;
        self.by_module.entry(module_id).or_default().push(def_id);
        self.by_id.insert(def_id, value);
    }

    pub fn insert_with_nominal_target(
        &mut self,
        module_id: ModuleId,
        value: ExtensionAssociatedValue,
        nominal_target: Option<GlobalDefId>,
    ) {
        let def_id = value.def_id;
        self.insert(module_id, value);
        if let Some(nominal_target) = nominal_target {
            self.by_nominal_target
                .entry(nominal_target)
                .or_default()
                .push(def_id);
        }
    }

    pub fn for_each_visible_value(
        &self,
        current_module: ModuleId,
        imported_modules: impl IntoIterator<Item = ModuleId>,
        visibility_allows: impl Fn(Visibility, ModuleId) -> bool,
        mut f: impl FnMut(&ExtensionAssociatedValue),
    ) {
        if let Some(value_ids) = self.by_module.get(&current_module) {
            for def_id in value_ids {
                if let Some(value) = self.by_id.get(def_id) {
                    f(value);
                }
            }
        }
        for module_id in imported_modules {
            if let Some(value_ids) = self.by_module.get(&module_id) {
                for def_id in value_ids {
                    let Some(value) = self.by_id.get(def_id) else {
                        continue;
                    };
                    if visibility_allows(value.visibility, value.def_id.module_id) {
                        f(value);
                    }
                }
            }
        }
    }

    pub fn visible_values(
        &self,
        current_module: ModuleId,
        imported_modules: impl IntoIterator<Item = ModuleId>,
        visibility_allows: impl Fn(Visibility, ModuleId) -> bool,
    ) -> Vec<ExtensionAssociatedValue> {
        let mut values = Vec::new();
        self.for_each_visible_value(
            current_module,
            imported_modules,
            visibility_allows,
            |value| values.push(value.clone()),
        );
        values
    }

    pub fn all_values(&self) -> impl Iterator<Item = &ExtensionAssociatedValue> {
        self.by_id.values()
    }

    pub fn values_for_nominal_target(
        &self,
        target_def_id: GlobalDefId,
    ) -> impl Iterator<Item = &ExtensionAssociatedValue> {
        self.by_nominal_target
            .get(&target_def_id)
            .into_iter()
            .flat_map(|value_ids| value_ids.iter())
            .filter_map(|def_id| self.by_id.get(def_id))
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
                .entry(method.name)
                .or_default()
                .push((target_index, method_index));
        }
        if method.is_trait_witness {
            self.trait_witnesses_by_name
                .entry(method.name)
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
            .entry((target_ty, value.name))
            .or_default()
            .push((target_index, value_index));
        self.targets[target_index].associated_values.push(value);
    }

    pub fn methods(&self, target_ty: InternedTyId, name: &SymbolId) -> Vec<GlobalDefId> {
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

    pub fn all_methods_named(
        &self,
        name: &SymbolId,
    ) -> Vec<(InternedTyId, VisibleExtensionMethod)> {
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
        name: &SymbolId,
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
        name: &SymbolId,
    ) -> Option<VisibleExtensionAssociatedValue> {
        let mut matches = self
            .associated_values_by_target_name
            .get(&(target_ty, *name))?
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
