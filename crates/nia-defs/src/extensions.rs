// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use crate::DefId;
use nia_ids::{GlobalDefId, InternedTyId, ModuleId, TraitId, TraitImplId, Visibility};
use nia_span::Span;
use nia_symbol::{SymbolId, SymbolMap};

/// Lowered `where` predicate attached to a signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WherePredicateSignature {
    /// Constrained type.
    pub ty: InternedTyId,
    /// Trait bounds applied to the type.
    pub bounds: Vec<WhereBoundSignature>,
    /// Predicate source span.
    pub span: Span,
}

/// One trait bound and its associated-type constraints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhereBoundSignature {
    /// Lowered trait instance type.
    pub trait_ty: InternedTyId,
    /// Associated-type equality bindings.
    pub associated_type_bindings: Vec<AssociatedTypeBindingSignature>,
    /// Bound source span.
    pub span: Span,
}

/// Associated-type equality in a trait bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssociatedTypeBindingSignature {
    /// Associated type name.
    pub name: SymbolId,
    /// Bound concrete type.
    pub ty: InternedTyId,
    /// Binding source span.
    pub span: Span,
}

/// Program extension methods indexed by module, target, name, and definition.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExtensionMethods {
    by_module: HashMap<ModuleId, Vec<GlobalDefId>>,
    by_nominal_target: HashMap<GlobalDefId, Vec<GlobalDefId>>,
    by_name: SymbolMap<Vec<GlobalDefId>>,
    by_id: HashMap<GlobalDefId, ExtensionMethod>,
}

/// Associated value declared by a trait or inherent extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionAssociatedValue {
    /// Associated value name.
    pub name: SymbolId,
    /// Global definition identity.
    pub def_id: GlobalDefId,
    /// Owning implementation identity.
    pub impl_id: TraitImplId,
    /// Extended target type.
    pub target_ty: InternedTyId,
    /// Implemented trait, or `None` for an inherent extension.
    pub trait_id: Option<TraitId>,
    /// Associated value visibility.
    pub visibility: Visibility,
}

/// Program extension associated values indexed by module and nominal target.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExtensionAssociatedValues {
    by_module: HashMap<ModuleId, Vec<GlobalDefId>>,
    by_nominal_target: HashMap<GlobalDefId, Vec<GlobalDefId>>,
    by_id: HashMap<GlobalDefId, ExtensionAssociatedValue>,
}

/// Associated value admitted into a module's visible extension set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleExtensionAssociatedValue {
    /// Associated value name.
    pub name: SymbolId,
    /// Global definition identity.
    pub def_id: GlobalDefId,
}

/// Method declared by a trait or inherent extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionMethod {
    /// Method name.
    pub name: SymbolId,
    /// Global method definition identity.
    pub def_id: GlobalDefId,
    /// Owning implementation identity.
    pub impl_id: TraitImplId,
    /// Type generic parameters inherited from the implementation and method.
    pub effective_generics: Vec<SymbolId>,
    /// Const generic parameters inherited from the implementation and method.
    pub effective_const_generics: Vec<SymbolId>,
    /// Extended target type.
    pub target_ty: InternedTyId,
    /// Implemented trait, or `None` for an inherent extension.
    pub trait_id: Option<TraitId>,
    /// Concrete or generic trait type arguments.
    pub trait_args: Vec<InternedTyId>,
    /// Const arguments of the implemented trait instance.
    pub trait_const_args: Vec<nia_ty::ConstGenericArg>,
    /// Predicates required for the method to apply.
    pub where_predicates: Vec<WherePredicateSignature>,
    /// Method visibility.
    pub visibility: Visibility,
}

/// Visibility-filtered extension members grouped by implementation and target.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VisibleExtensionMethods {
    targets: Vec<VisibleExtensionTarget>,
    callable_by_name: SymbolMap<Vec<(usize, usize)>>,
    trait_witnesses_by_name: SymbolMap<Vec<(usize, usize)>>,
    trait_witness_impls: HashSet<(ModuleId, TraitImplId)>,
    associated_values_by_target_name: HashMap<(InternedTyId, SymbolId), Vec<(usize, usize)>>,
}

/// Extension method with separate callability and trait-witness capabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleExtensionMethod {
    /// Method name.
    pub name: SymbolId,
    /// Global method definition identity.
    pub def_id: GlobalDefId,
    /// Owning implementation identity.
    pub impl_id: TraitImplId,
    /// Effective type generic parameters.
    pub effective_generics: Vec<SymbolId>,
    /// Effective const generic parameters.
    pub effective_const_generics: Vec<SymbolId>,
    /// Implemented trait, or `None` for an inherent extension.
    pub trait_id: Option<TraitId>,
    /// Trait type arguments.
    pub trait_args: Vec<InternedTyId>,
    /// Const arguments of the implemented trait instance.
    pub trait_const_args: Vec<nia_ty::ConstGenericArg>,
    /// Predicates required for the method to apply.
    pub where_predicates: Vec<WherePredicateSignature>,
    /// Whether ordinary method lookup may call this method.
    pub is_callable: bool,
    /// Whether this method may witness a visible trait obligation.
    pub is_trait_witness: bool,
}

/// Visible members for one implementation and normalized target type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleExtensionTarget {
    /// Implementation identity.
    pub impl_id: TraitImplId,
    /// Normalized extended target type.
    pub target_ty: InternedTyId,
    /// Visible methods in insertion order.
    pub methods: Vec<VisibleExtensionMethod>,
    /// Visible associated values in insertion order.
    pub associated_values: Vec<VisibleExtensionAssociatedValue>,
}

impl ExtensionMethods {
    /// Merges all indexes from `other` while preserving insertion order.
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

    /// Inserts a method into module, name, and id indexes.
    pub fn insert(&mut self, module_id: ModuleId, method: ExtensionMethod) {
        let def_id = method.def_id;
        self.by_name.entry(method.name).or_default().push(def_id);
        self.by_module.entry(module_id).or_default().push(def_id);
        self.by_id.insert(def_id, method);
    }

    /// Inserts a method and optionally indexes its canonical nominal target.
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

    /// Visits local and imported methods admitted by the visibility predicate.
    ///
    /// Imported trait methods remain candidates independent of member
    /// visibility because their callability is decided by trait visibility.
    /// Repeated imported modules and the current module are visited only once.
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
        let mut visited_modules = HashSet::from([current_module]);
        for module_id in imported_modules {
            if !visited_modules.insert(module_id) {
                continue;
            }
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

    /// Collects the methods visited by [`Self::for_each_visible_method`].
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

    /// Returns module-local method ids in insertion order.
    pub fn method_ids_for_module(&self, module_id: ModuleId) -> Vec<DefId> {
        self.by_module
            .get(&module_id)
            .into_iter()
            .flat_map(|method_ids| method_ids.iter())
            .map(|def_id| def_id.def_id)
            .collect()
    }

    /// Iterates every indexed method.
    pub fn all_methods(&self) -> impl Iterator<Item = &ExtensionMethod> {
        self.by_id.values()
    }

    /// Iterates methods with the requested name.
    pub fn methods_named(&self, name: &SymbolId) -> impl Iterator<Item = &ExtensionMethod> {
        self.by_name
            .get(name)
            .into_iter()
            .flat_map(|method_ids| method_ids.iter())
            .filter_map(|def_id| self.by_id.get(def_id))
    }

    /// Looks up a method by global definition id.
    pub fn method_by_id(&self, def_id: GlobalDefId) -> Option<&ExtensionMethod> {
        self.by_id.get(&def_id)
    }

    /// Iterates methods indexed for a canonical nominal target.
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
    /// Merges all indexes from `other` while preserving insertion order.
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

    /// Inserts an associated value into module and id indexes.
    pub fn insert(&mut self, module_id: ModuleId, value: ExtensionAssociatedValue) {
        let def_id = value.def_id;
        self.by_module.entry(module_id).or_default().push(def_id);
        self.by_id.insert(def_id, value);
    }

    /// Inserts a value and optionally indexes its canonical nominal target.
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

    /// Visits local and imported associated values admitted by visibility.
    /// Repeated imported modules and the current module are visited only once.
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
        let mut visited_modules = HashSet::from([current_module]);
        for module_id in imported_modules {
            if !visited_modules.insert(module_id) {
                continue;
            }
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

    /// Collects the values visited by [`Self::for_each_visible_value`].
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

    /// Iterates every indexed associated value.
    pub fn all_values(&self) -> impl Iterator<Item = &ExtensionAssociatedValue> {
        self.by_id.values()
    }

    /// Iterates values indexed for a canonical nominal target.
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
    /// Inserts one method under an implementation and normalized target.
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

    /// Marks an implementation as an available trait witness.
    pub fn insert_trait_witness_impl(&mut self, module_id: ModuleId, impl_id: TraitImplId) {
        self.trait_witness_impls.insert((module_id, impl_id));
    }

    /// Inserts an associated value under an implementation and target.
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

    /// Returns callable method ids for an exact normalized target and name.
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

    /// Returns callable methods with `name` across all targets.
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

    /// Returns trait-witness methods with `name` across all targets.
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

    /// Returns grouped visible extension targets in insertion order.
    pub fn targets(&self) -> &[VisibleExtensionTarget] {
        &self.targets
    }

    /// Tests whether an implementation is available as a trait witness.
    pub fn has_trait_witness_impl(&self, module_id: ModuleId, impl_id: TraitImplId) -> bool {
        self.trait_witness_impls.contains(&(module_id, impl_id))
    }

    /// Iterates available trait-witness implementation identities.
    pub fn trait_witness_impls(&self) -> impl Iterator<Item = (ModuleId, TraitImplId)> + '_ {
        self.trait_witness_impls.iter().copied()
    }

    /// Resolves a unique associated value for an exact target and name.
    ///
    /// Returns `None` when no value exists or multiple implementations make
    /// the lookup ambiguous.
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

#[cfg(test)]
mod tests {
    use super::*;
    use nia_ids::ModuleIdAllocator;
    use nia_symbol::known;
    use nia_ty::{PrimitiveTy, TypeStore};

    #[test]
    fn visible_extension_iteration_deduplicates_module_inputs() {
        let mut module_ids = ModuleIdAllocator::new();
        let current = module_ids.allocate();
        let imported = module_ids.allocate();
        let type_store = TypeStore::new();
        let target_ty = type_store
            .append_for_module(current)
            .primitive(PrimitiveTy::I32);
        let current_id = GlobalDefId {
            module_id: current,
            def_id: DefId(1),
        };
        let imported_id = GlobalDefId {
            module_id: imported,
            def_id: DefId(2),
        };
        let mut methods = ExtensionMethods::default();
        for (module_id, def_id) in [(current, current_id), (imported, imported_id)] {
            methods.insert(
                module_id,
                ExtensionMethod {
                    name: known::ITEM,
                    def_id,
                    impl_id: TraitImplId(def_id.def_id.0),
                    effective_generics: Vec::new(),
                    effective_const_generics: Vec::new(),
                    target_ty,
                    trait_id: None,
                    trait_args: Vec::new(),
                    trait_const_args: Vec::new(),
                    where_predicates: Vec::new(),
                    visibility: Visibility::Public,
                },
            );
        }
        let mut values = ExtensionAssociatedValues::default();
        for (module_id, def_id) in [(current, current_id), (imported, imported_id)] {
            values.insert(
                module_id,
                ExtensionAssociatedValue {
                    name: known::ITEM,
                    def_id,
                    impl_id: TraitImplId(def_id.def_id.0),
                    target_ty,
                    trait_id: None,
                    visibility: Visibility::Public,
                },
            );
        }
        let repeated_modules = [current, current, imported, imported];

        let visible_methods = methods.visible_methods(current, repeated_modules, |_, _| true);
        let visible_values = values.visible_values(current, repeated_modules, |_, _| true);

        assert_eq!(
            visible_methods
                .iter()
                .map(|method| method.def_id)
                .collect::<Vec<_>>(),
            [current_id, imported_id]
        );
        assert_eq!(
            visible_values
                .iter()
                .map(|value| value.def_id)
                .collect::<Vec<_>>(),
            [current_id, imported_id]
        );
    }
}
