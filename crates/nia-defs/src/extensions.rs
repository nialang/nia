// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::DefId;
use nia_ast::Visibility;
use nia_ids::{GlobalDefId, InternedTyId, ModuleId, TraitId};
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
pub struct ExtensionMethod {
    pub name: String,
    pub def_id: GlobalDefId,
    pub impl_index: usize,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleExtensionMethod {
    pub name: String,
    pub def_id: GlobalDefId,
    pub impl_index: usize,
    pub impl_generics: Vec<String>,
    pub trait_id: Option<TraitId>,
    pub trait_args: Vec<InternedTyId>,
    pub where_predicates: Vec<WherePredicateSignature>,
    pub is_callable: bool,
    pub is_trait_witness: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleExtensionTarget {
    pub impl_index: usize,
    pub target_ty: InternedTyId,
    pub methods: Vec<VisibleExtensionMethod>,
}

impl ExtensionMethods {
    pub fn insert(&mut self, module_id: ModuleId, method: ExtensionMethod) {
        self.by_module.entry(module_id).or_default().push(method);
    }

    pub fn visible_methods(
        &self,
        current_module: ModuleId,
        imported_modules: impl IntoIterator<Item = ModuleId>,
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
                            method.visibility == Visibility::Public || method.trait_id.is_some()
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

impl VisibleExtensionMethods {
    pub fn insert(
        &mut self,
        impl_index: usize,
        target_ty: InternedTyId,
        method: VisibleExtensionMethod,
    ) {
        if let Some(existing) = self
            .targets
            .iter_mut()
            .find(|item| item.impl_index == impl_index && item.target_ty == target_ty)
        {
            existing.methods.push(method);
            return;
        }
        self.targets.push(VisibleExtensionTarget {
            impl_index,
            target_ty,
            methods: vec![method],
        });
    }

    pub fn methods(&self, target_ty: InternedTyId, name: &str) -> Vec<GlobalDefId> {
        self.targets
            .iter()
            .filter(|item| item.target_ty == target_ty)
            .flat_map(|item| item.methods.iter())
            .filter(|method| method.name == name)
            .map(|method| method.def_id)
            .collect()
    }

    pub fn all_methods_named(&self, name: &str) -> Vec<(InternedTyId, VisibleExtensionMethod)> {
        self.targets
            .iter()
            .flat_map(|item| {
                item.methods
                    .iter()
                    .filter(|method| method.is_callable)
                    .filter(move |method| method.name == name)
                    .cloned()
                    .map(move |method| (item.target_ty, method))
            })
            .collect()
    }

    pub fn all_trait_witnesses_named(
        &self,
        name: &str,
    ) -> Vec<(InternedTyId, VisibleExtensionMethod)> {
        self.targets
            .iter()
            .flat_map(|item| {
                item.methods
                    .iter()
                    .filter(|method| method.is_trait_witness)
                    .filter(move |method| method.name == name)
                    .cloned()
                    .map(move |method| (item.target_ty, method))
            })
            .collect()
    }

    pub fn targets(&self) -> &[VisibleExtensionTarget] {
        &self.targets
    }
}
