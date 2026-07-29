// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub struct ExecutableExtensionIndex<'a> {
    by_trait: nia_hash::FastHashMap<TraitId, Vec<&'a nia_defs::ExtensionMethod>>,
    by_trait_method: nia_hash::FastHashMap<(TraitId, SymbolId), Vec<&'a nia_defs::ExtensionMethod>>,
    where_predicates_by_def:
        nia_hash::FastHashMap<GlobalDefId, &'a [nia_defs::WherePredicateSignature]>,
    trait_impls_by_key:
        nia_hash::FastHashMap<(ModuleId, TraitImplId, TraitId), &'a ProgramTraitImplSignature>,
}

pub trait ExecutableExtensionLookup {
    fn for_each_method_for_trait(
        &self,
        trait_id: TraitId,
        f: &mut dyn FnMut(&nia_defs::ExtensionMethod),
    );

    fn for_each_method_for_trait_method(
        &self,
        trait_id: TraitId,
        method_name: &SymbolId,
        f: &mut dyn FnMut(&nia_defs::ExtensionMethod),
    );

    fn with_where_predicates_for_def(
        &self,
        def_id: GlobalDefId,
        f: &mut dyn FnMut(&[nia_defs::WherePredicateSignature]),
    );

    fn with_trait_impl_for_method(
        &self,
        method: &nia_defs::ExtensionMethod,
        trait_id: TraitId,
        f: &mut dyn FnMut(&ProgramTraitImplSignature),
    ) -> bool;
}

impl<'a> ExecutableExtensionIndex<'a> {
    pub fn new(
        extension_methods: &'a ExtensionMethods,
        trait_impls: &'a [ProgramTraitImplSignature],
    ) -> Self {
        let mut by_trait =
            nia_hash::FastHashMap::<TraitId, Vec<&'a nia_defs::ExtensionMethod>>::default();
        let mut by_trait_method = nia_hash::FastHashMap::<
            (TraitId, SymbolId),
            Vec<&'a nia_defs::ExtensionMethod>,
        >::default();
        let mut where_predicates_by_def =
            nia_hash::FastHashMap::<GlobalDefId, &'a [nia_defs::WherePredicateSignature]>::default(
            );
        let trait_impls_by_key = trait_impls
            .iter()
            .map(|impl_signature| {
                (
                    (
                        impl_signature.module_id,
                        impl_signature.impl_id,
                        impl_signature.trait_id,
                    ),
                    impl_signature,
                )
            })
            .collect::<nia_hash::FastHashMap<_, _>>();
        for method in extension_methods.all_methods() {
            where_predicates_by_def.insert(method.def_id, method.where_predicates.as_slice());
            if let Some(trait_id) = method.trait_id {
                by_trait.entry(trait_id).or_default().push(method);
                by_trait_method
                    .entry((trait_id, method.name))
                    .or_default()
                    .push(method);
            }
        }
        Self {
            by_trait,
            by_trait_method,
            where_predicates_by_def,
            trait_impls_by_key,
        }
    }
}

impl ExecutableExtensionLookup for ExecutableExtensionIndex<'_> {
    fn for_each_method_for_trait(
        &self,
        trait_id: TraitId,
        f: &mut dyn FnMut(&nia_defs::ExtensionMethod),
    ) {
        if let Some(methods) = self.by_trait.get(&trait_id) {
            for method in methods {
                f(method);
            }
        }
    }

    fn for_each_method_for_trait_method(
        &self,
        trait_id: TraitId,
        method_name: &SymbolId,
        f: &mut dyn FnMut(&nia_defs::ExtensionMethod),
    ) {
        if let Some(methods) = self.by_trait_method.get(&(trait_id, *method_name)) {
            for method in methods {
                f(method);
            }
        }
    }

    fn with_where_predicates_for_def(
        &self,
        def_id: GlobalDefId,
        f: &mut dyn FnMut(&[nia_defs::WherePredicateSignature]),
    ) {
        let predicates = self
            .where_predicates_by_def
            .get(&def_id)
            .copied()
            .unwrap_or(&[]);
        f(predicates);
    }

    fn with_trait_impl_for_method(
        &self,
        method: &nia_defs::ExtensionMethod,
        trait_id: TraitId,
        f: &mut dyn FnMut(&ProgramTraitImplSignature),
    ) -> bool {
        let Some(signature) =
            self.trait_impls_by_key
                .get(&(method.def_id.module_id, method.impl_id, trait_id))
        else {
            return false;
        };
        f(signature);
        true
    }
}
