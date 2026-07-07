// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_ids::GlobalDefId;
use nia_symbol::SymbolId;
use nia_trait_solve::TraitResolution;

use crate::{
    BackendLowerModuleInput, BuiltinTraitGoalKey, ExtensionTraitMethodCandidate,
    ExtensionTraitMethodKey, index_extension_trait_method_candidates,
    index_local_method_symbols_by_def, index_local_trait_impls_by_method,
    index_local_trait_methods_with_defaults, trait_object_vtables,
};

pub(crate) struct BackendTraitContext {
    pub(crate) trait_impls_by_method: HashMap<GlobalDefId, usize>,
    pub(crate) extension_trait_method_candidates:
        HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>>,
    pub(crate) builtin_trait_resolutions: HashMap<BuiltinTraitGoalKey, TraitResolution>,
    pub(crate) trait_methods_with_defaults: HashSet<GlobalDefId>,
    pub(crate) method_symbols_by_def: HashMap<GlobalDefId, SymbolId>,
    pub(crate) trait_object_vtables: trait_object_vtables::TraitObjectVtableCache,
}

impl BackendTraitContext {
    pub(crate) fn new(input: &BackendLowerModuleInput<'_>) -> Self {
        Self {
            extension_trait_method_candidates: index_extension_trait_method_candidates(
                input.extensions,
                input.extension_interner.unwrap_or(input.function_interner),
            ),
            builtin_trait_resolutions: HashMap::new(),
            trait_impls_by_method: index_local_trait_impls_by_method(input),
            trait_methods_with_defaults: index_local_trait_methods_with_defaults(input),
            method_symbols_by_def: index_local_method_symbols_by_def(input),
            trait_object_vtables: trait_object_vtables::TraitObjectVtableCache::default(),
        }
    }
}
