// SPDX-License-Identifier: GPL-3.0-or-later
//! Cross-module lookup indexes built once for backend lowering.

use super::*;

pub(crate) fn index_extension_generics_by_method(
    extensions: &ExtensionMethods,
) -> HashMap<GlobalDefId, Vec<SymbolId>> {
    let mut generics_by_method = HashMap::new();
    for method in extensions.all_methods() {
        generics_by_method.insert(method.def_id, method.effective_generics.clone());
    }
    generics_by_method
}

pub(crate) fn index_local_extension_generics_by_method(
    extensions: &VisibleExtensionMethods,
) -> HashMap<GlobalDefId, Vec<SymbolId>> {
    let mut generics_by_method = HashMap::new();
    for target in extensions.targets() {
        for method in &target.methods {
            generics_by_method.insert(method.def_id, method.effective_generics.clone());
        }
    }
    generics_by_method
}

pub(crate) fn index_local_extension_method_sources_by_def(
    input: &BackendLowerModuleInput<'_>,
) -> HashMap<GlobalDefId, ExtensionMethodSource> {
    let mut sources = HashMap::new();
    for target in input.extensions.targets() {
        for method in &target.methods {
            sources.insert(
                method.def_id,
                ExtensionMethodSource {
                    module_id: input.module_id,
                    target_ty: target.target_ty,
                    where_predicates: method.where_predicates.clone(),
                },
            );
        }
    }
    sources
}

pub(crate) fn index_program_extension_method_sources_by_def(
    input: &BackendLowerModuleInput<'_>,
) -> HashMap<GlobalDefId, ExtensionMethodSource> {
    let mut sources = HashMap::new();
    for method in input.program.extension_methods().all_methods() {
        sources.insert(
            method.def_id,
            ExtensionMethodSource {
                module_id: method.def_id.module_id,
                target_ty: method.target_ty,
                where_predicates: method.where_predicates.clone(),
            },
        );
    }
    sources
}

pub(crate) fn index_local_trait_impls_by_method(
    input: &BackendLowerModuleInput<'_>,
) -> HashMap<GlobalDefId, usize> {
    let impls = input
        .program
        .trait_impls()
        .iter()
        .enumerate()
        .map(|(program_index, impl_signature)| {
            (
                (impl_signature.module_id, impl_signature.impl_id),
                program_index,
            )
        })
        .collect::<HashMap<_, _>>();
    let mut impls_by_method = HashMap::new();
    for target in input.extensions.targets() {
        for method in &target.methods {
            let Some(program_index) = impls.get(&(input.module_id, method.impl_id)).copied() else {
                continue;
            };
            impls_by_method.insert(method.def_id, program_index);
        }
    }
    impls_by_method
}

pub(crate) fn index_program_trait_impls_by_method(
    input: &BackendLowerModuleInput<'_>,
) -> HashMap<GlobalDefId, usize> {
    let impls = input
        .program
        .trait_impls()
        .iter()
        .enumerate()
        .map(|(program_index, impl_signature)| {
            (
                (impl_signature.module_id, impl_signature.impl_id),
                program_index,
            )
        })
        .collect::<HashMap<_, _>>();
    let mut impls_by_method = HashMap::new();
    for method in input.program.extension_methods().all_methods() {
        let Some(program_index) = impls
            .get(&(method.def_id.module_id, method.impl_id))
            .copied()
        else {
            continue;
        };
        impls_by_method.insert(method.def_id, program_index);
    }
    impls_by_method
}

pub(crate) fn index_extension_trait_method_candidates(
    extensions: &VisibleExtensionMethods,
    module_id: ModuleId,
) -> HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>> {
    let mut candidates: HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>> =
        HashMap::new();
    for target in extensions.targets() {
        for method in &target.methods {
            if !method.is_trait_witness {
                continue;
            }
            let Some(trait_id) = method.trait_id else {
                continue;
            };
            candidates
                .entry(ExtensionTraitMethodKey {
                    trait_id,
                    method_name: method.name,
                    trait_arg_count: method.trait_args.len(),
                    trait_const_arg_count: method.trait_const_args.len(),
                })
                .or_default()
                .push(ExtensionTraitMethodCandidate {
                    module_id,
                    target_ty: target.target_ty,
                    method_def_id: method.def_id,
                    trait_args: method.trait_args.clone(),
                    trait_const_args: method.trait_const_args.clone(),
                    where_predicates: method.where_predicates.clone(),
                    effective_generics: method.effective_generics.clone(),
                });
        }
    }
    candidates
}

pub(crate) fn index_program_extension_trait_method_candidates(
    input: Option<&BackendLowerModuleInput<'_>>,
) -> HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>> {
    let Some(input) = input else {
        return HashMap::new();
    };
    let impls = input
        .program
        .trait_impls()
        .iter()
        .map(|impl_signature| {
            (
                (impl_signature.module_id, impl_signature.impl_id),
                impl_signature,
            )
        })
        .collect::<HashMap<_, _>>();
    let mut candidates: HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>> =
        HashMap::new();
    for method in input.program.extension_methods().all_methods() {
        let Some(trait_id) = method.trait_id else {
            continue;
        };
        let Some(impl_signature) = impls.get(&(method.def_id.module_id, method.impl_id)) else {
            continue;
        };
        let candidate = ExtensionTraitMethodCandidate {
            module_id: impl_signature.module_id,
            target_ty: impl_signature.target_ty,
            method_def_id: method.def_id,
            trait_args: impl_signature.trait_args.clone(),
            trait_const_args: impl_signature.trait_const_args.clone(),
            where_predicates: impl_signature.where_predicates.clone(),
            effective_generics: impl_signature.generics.clone(),
        };
        candidates
            .entry(ExtensionTraitMethodKey {
                trait_id,
                method_name: method.name,
                trait_arg_count: method.trait_args.len(),
                trait_const_arg_count: impl_signature.trait_const_args.len(),
            })
            .or_default()
            .push(candidate);
    }
    for bucket in candidates.values_mut() {
        let mut seen = HashSet::new();
        bucket.retain(|candidate| {
            if seen.contains(&candidate.method_def_id) {
                false
            } else {
                seen.insert(candidate.method_def_id);
                true
            }
        });
    }
    candidates
}

pub(crate) fn index_local_trait_methods_with_defaults(
    input: &BackendLowerModuleInput<'_>,
) -> HashSet<GlobalDefId> {
    input
        .signatures
        .traits
        .values()
        .flat_map(|signature| signature.methods.iter())
        .filter(|method| method.has_default)
        .map(|method| GlobalDefId {
            module_id: input.module_id,
            def_id: method.def_id,
        })
        .collect::<HashSet<_>>()
}

pub(crate) fn index_program_trait_methods_with_defaults(
    input: &BackendLowerModuleInput<'_>,
) -> HashSet<GlobalDefId> {
    input
        .program
        .traits()
        .iter()
        .flat_map(|(trait_id, signature)| {
            signature
                .signature
                .methods
                .iter()
                .filter(|method| method.has_default)
                .map(|method| GlobalDefId {
                    module_id: trait_id.module_id,
                    def_id: method.def_id,
                })
        })
        .collect()
}

pub(crate) fn index_local_method_symbols_by_def(
    input: &BackendLowerModuleInput<'_>,
) -> HashMap<GlobalDefId, SymbolId> {
    let mut names = input
        .defs
        .defs
        .iter()
        .map(|(def_id, def)| {
            (
                GlobalDefId {
                    module_id: input.module_id,
                    def_id,
                },
                def.name,
            )
        })
        .collect::<HashMap<_, _>>();
    for target in input.extensions.targets() {
        for method in &target.methods {
            names.entry(method.def_id).or_insert(method.name);
        }
    }
    names
}

pub(crate) fn index_program_method_symbols_by_def(
    input: &BackendLowerModuleInput<'_>,
) -> HashMap<GlobalDefId, SymbolId> {
    let mut names = HashMap::new();
    for (trait_id, signature) in input.program.traits() {
        for method in &signature.signature.methods {
            names.insert(
                GlobalDefId {
                    module_id: trait_id.module_id,
                    def_id: method.def_id,
                },
                method.name,
            );
        }
    }
    for method in input.program.extension_methods().all_methods() {
        names.insert(method.def_id, method.name);
    }
    names
}

pub(crate) fn index_layout_instances_by_def<'a>(
    keys: impl IntoIterator<Item = &'a StructLayoutKey>,
) -> HashMap<DefId, Vec<StructLayoutKey>> {
    let mut instances_by_def = HashMap::new();
    for key in keys {
        instances_by_def
            .entry(key.def_id)
            .or_insert_with(Vec::new)
            .push(key.clone());
    }
    instances_by_def
}
