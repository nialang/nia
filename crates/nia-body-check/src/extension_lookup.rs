// SPDX-License-Identifier: GPL-3.0-or-later
use std::cell::OnceCell;

use super::*;

#[derive(Debug, Clone)]
pub(super) struct ExtensionMethodLookup {
    pub(super) target_ty: InternedTyId,
    pub(super) impl_id: nia_ids::TraitImplId,
    pub(super) effective_generics: Vec<SymbolId>,
    pub(super) effective_const_generics: Vec<SymbolId>,
    pub(super) where_predicates: Vec<nia_defs::WherePredicateSignature>,
}

pub(super) enum BodyVisibleExtensionSource<'a> {
    Eager(VisibleExtensionMethods),
    Lazy {
        load: &'a dyn Fn() -> VisibleExtensionMethods,
        loaded: OnceCell<VisibleExtensionMethods>,
    },
}

impl Clone for BodyVisibleExtensionSource<'_> {
    fn clone(&self) -> Self {
        match self {
            Self::Eager(methods) => Self::Eager(methods.clone()),
            Self::Lazy { load, loaded } => {
                let cloned = OnceCell::new();
                if let Some(methods) = loaded.get() {
                    cloned.get_or_init(|| methods.clone());
                }
                Self::Lazy {
                    load: *load,
                    loaded: cloned,
                }
            }
        }
    }
}

impl<'a> BodyVisibleExtensionSource<'a> {
    fn with_methods<T>(&self, f: impl FnOnce(&VisibleExtensionMethods) -> T) -> T {
        match self {
            Self::Eager(methods) => f(methods),
            Self::Lazy { load, loaded } => f(loaded.get_or_init(load)),
        }
    }
}

impl<'a> BodyChecker<'a> {
    pub(super) fn with_visible_extensions<T>(
        &mut self,
        f: impl FnOnce(&VisibleExtensionMethods) -> T,
    ) -> T {
        self.extensions.with_methods(f)
    }

    pub(super) fn visible_extension_trait_witness_impls(
        &mut self,
    ) -> HashSet<(ModuleId, nia_ids::TraitImplId)> {
        self.with_visible_extensions(|extensions| extensions.trait_witness_impls().collect())
    }

    pub(super) fn extension_method_lookup(
        module_id: ModuleId,
        defs: &DefCollection,
        signatures: BodyLocalSignatures<'_>,
        extensions: BodyVisibleExtensions<'_>,
        local_normalization: &TypeNormalization,
    ) -> Arc<HashMap<GlobalDefId, ExtensionMethodLookup>> {
        let mut methods = HashMap::new();
        for impl_signature in signatures.trait_impls {
            if impl_signature.builtin.is_some() {
                continue;
            }
            let target_ty = local_normalization.normalize(impl_signature.target_ty);
            for method in &impl_signature.methods {
                let mut effective_generics = impl_signature.generics.clone();
                let mut effective_const_generics = impl_signature
                    .generic_params
                    .iter()
                    .filter_map(|generic| {
                        matches!(
                            generic.kind,
                            nia_item_signatures::GenericParamSignatureKind::Const { .. }
                        )
                        .then_some(generic.name)
                    })
                    .collect::<Vec<_>>();
                if let Some(def) = defs.defs.get(method.def_id) {
                    effective_generics.extend(def.generics.iter().cloned());
                    effective_const_generics.extend(def.const_generic_names());
                }
                methods.insert(
                    GlobalDefId {
                        module_id,
                        def_id: method.def_id,
                    },
                    ExtensionMethodLookup {
                        target_ty,
                        impl_id: impl_signature.impl_id,
                        effective_generics,
                        effective_const_generics,
                        where_predicates: impl_signature.where_predicates.clone(),
                    },
                );
            }
        }
        if extensions.lazy.is_some() {
            return Arc::new(methods);
        }
        for target in extensions.methods.targets() {
            let target_ty = target.target_ty;
            for method in &target.methods {
                methods
                    .entry(method.def_id)
                    .or_insert_with(|| ExtensionMethodLookup {
                        target_ty,
                        impl_id: method.impl_id,
                        effective_generics: method.effective_generics.clone(),
                        effective_const_generics: method.effective_const_generics.clone(),
                        where_predicates: method.where_predicates.clone(),
                    });
            }
        }
        Arc::new(methods)
    }

    pub(super) fn extension_method_lookup_for_id(
        &self,
        method_id: GlobalDefId,
    ) -> Option<&ExtensionMethodLookup> {
        self.extension_method_lookup_cache
            .get(&method_id)
            .or_else(|| self.extension_methods_by_id.get(&method_id))
    }

    pub(super) fn ensure_extension_method_lookup_for_id(
        &mut self,
        method_id: GlobalDefId,
    ) -> Option<&ExtensionMethodLookup> {
        if self.extension_method_lookup_for_id(method_id).is_none()
            && let Some(method_by_id) = self.program.extension_method_by_id
            && let Some(method) = method_by_id(method_id)
            && let Some(lookup) = self.program_extension_method_lookup(&method)
        {
            self.extension_method_lookup_cache.insert(method_id, lookup);
        }
        self.extension_method_lookup_for_id(method_id)
    }

    pub(super) fn program_extension_method_lookup(
        &mut self,
        method: &nia_defs::ExtensionMethod,
    ) -> Option<ExtensionMethodLookup> {
        if method.def_id.module_id != self.defs.module_id && method.visibility != Visibility::Public
        {
            return None;
        }
        let program_normalizations = self.program.extension_type_normalizations?;
        let normalization = program_normalizations(method.def_id.module_id)?;
        let target_ty = normalization.normalize(method.target_ty);
        Some(ExtensionMethodLookup {
            target_ty,
            impl_id: method.impl_id,
            effective_generics: method.effective_generics.clone(),
            effective_const_generics: method.effective_const_generics.clone(),
            where_predicates: method.where_predicates.clone(),
        })
    }
}
