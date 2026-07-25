// SPDX-License-Identifier: GPL-3.0-or-later
use super::program_signatures::program_signature_facts;
use super::*;
use nia_symbol::ToSymbolId;
use std::cell::RefCell;

struct SharedProgramDefsResolver<'a> {
    db: &'a QueryDb<CompilerContext>,
    cache: RefCell<HashMap<ModuleId, Option<Arc<DefCollection>>>>,
}

impl<'a> SharedProgramDefsResolver<'a> {
    fn new(db: &'a QueryDb<CompilerContext>) -> Self {
        Self {
            db,
            cache: RefCell::new(HashMap::new()),
        }
    }
}

impl nia_program_signatures::ProgramDefsResolver for SharedProgramDefsResolver<'_> {
    fn defs(&self, module_id: ModuleId) -> Option<Arc<DefCollection>> {
        if let Some(defs) = self.cache.borrow().get(&module_id) {
            return defs.clone();
        }
        let defs = Some(self.db.get(ModuleDefsQuery(module_id)));
        self.cache.borrow_mut().insert(module_id, defs.clone());
        defs
    }
}

pub(super) fn provide_extension_provider_summary(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_provider_summary::ProviderSummary {
    db.context().module_provider_summary(db, module_id)
}

pub(super) fn provide_extension_provider_discovery_index(
    db: &QueryDb<CompilerContext>,
) -> ExtensionProviderDiscoveryIndexValue {
    let timings = db.context().timings();
    let parse_ok_modules = resolve_stable_module_sequence(db, &db.get(ParseOkModuleIdsQuery));
    let trait_modules = parse_ok_modules
        .into_iter()
        .filter(|module_id| *db.get(ExtensionProviderModuleEligibilityQuery(*module_id)))
        .collect::<Vec<_>>();
    time_provider(timings, "extension_provider_discovery_index", || {
        let mut provider_modules = Vec::new();
        let mut nominal_candidates_by_name: HashMap<SymbolId, Vec<ModuleId>> = HashMap::new();
        let mut method_candidates_by_name: HashMap<SymbolId, Vec<ModuleId>> = HashMap::new();
        let mut trait_impl_candidates_by_name: HashMap<SymbolId, Vec<ModuleId>> = HashMap::new();
        for module_id in trait_modules {
            let summary = db.get(ExtensionProviderSummaryQuery(module_id));
            if !summary.has_providers() {
                continue;
            }
            provider_modules.push(module_id);
            for name in summary.nominal_provider_index_names() {
                nominal_candidates_by_name
                    .entry(name)
                    .or_default()
                    .push(module_id);
            }
            for name in summary.method_index_names() {
                method_candidates_by_name
                    .entry(name)
                    .or_default()
                    .push(module_id);
            }
            for name in summary.trait_impl_index_names() {
                trait_impl_candidates_by_name
                    .entry(name)
                    .or_default()
                    .push(module_id);
            }
        }
        provider_modules.sort();
        provider_modules.dedup();
        for modules in nominal_candidates_by_name.values_mut() {
            modules.sort();
            modules.dedup();
        }
        for modules in method_candidates_by_name.values_mut() {
            modules.sort();
            modules.dedup();
        }
        for modules in trait_impl_candidates_by_name.values_mut() {
            modules.sort();
            modules.dedup();
        }
        ExtensionProviderDiscoveryIndexQueryValue {
            provider_modules,
            nominal_candidates_by_name,
            method_candidates_by_name,
            trait_impl_candidates_by_name,
        }
    })
}

pub(super) fn provide_extension_provider_module_ids(
    db: &QueryDb<CompilerContext>,
) -> StableModuleSequence {
    let parse_ok_modules =
        resolve_stable_module_sequence_from_current_inputs(db, &db.get(ParseOkModuleIdsQuery));
    let module_ids = parse_ok_modules
        .into_iter()
        .filter(|module_id| *db.get(ExtensionProviderModuleEligibilityQuery(*module_id)))
        .collect::<Vec<_>>();
    stable_module_sequence(db, module_ids)
}

pub(super) fn provide_extension_provider_module_eligibility(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> bool {
    db.get(ExtensionProviderSummaryQuery(module_id))
        .has_providers()
}

pub(super) fn provide_extension_signature_module_input(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ExtensionSignatureModuleInputValue {
    ExtensionSignatureModuleInputQueryValue {
        module_id,
        defs: db.get(ModuleDefsQuery(module_id)),
        lowering: db.get(SignatureTypeLoweringQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        )),
        signatures: db.get(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        )),
        function_signatures: db.get(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::ExtensionFunctions,
        )),
        type_signatures: db.get(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Types,
        )),
        normalization: db.get(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        )),
    }
}

pub(super) fn provide_extension_trait_solving_module_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ExtensionTraitSolvingModuleFactsValue {
    let program_sources = db.get(FrontendProgramSourcesQuery);
    let cache_input = program_sources
        .as_ref()
        .as_ref()
        .and_then(|program_sources| {
            let source = program_sources.by_module.get(&module_id)?;
            let namespace = crate::FrontendCacheNamespace::new(
                &db.context().loader_facts.target(),
                db.context().loader_facts.runtime(),
            );
            let key = crate::FrontendExtensionTraitSolvingFactsCacheKey::new(
                namespace,
                &source.module,
                program_sources.fingerprint,
            );
            Some((program_sources, source, namespace, key))
        });
    let symbols = db.context().symbols();
    let cached = if let Some(cache) = db.context().signature_cache.as_ref()
        && let Some((program_sources, source, namespace, key)) = cache_input
    {
        match cache.load_extension_trait_solving_facts(
            crate::signature_cache::ExtensionTraitSolvingFactsIdentity {
                key,
                namespace,
                module: &source.module,
                program_sources: program_sources.fingerprint,
                source_len: source.len,
            },
            &program_sources.module_by_path,
            &symbols,
            db.context().type_store(),
        ) {
            Ok(lookup) => {
                match lookup {
                    crate::signature_cache::ExtensionTraitSolvingFactsLookup::Hit(_) => {
                        nia_timing::emit_counter(
                            "frontend.extension_trait_solving_facts_reuse_hits",
                            1,
                        );
                    }
                    crate::signature_cache::ExtensionTraitSolvingFactsLookup::NotFound => {
                        nia_timing::emit_counter(
                            "frontend.extension_trait_solving_facts_reuse_miss_not_found",
                            1,
                        );
                    }
                    crate::signature_cache::ExtensionTraitSolvingFactsLookup::Corrupt => {
                        nia_timing::emit_counter(
                            "frontend.extension_trait_solving_facts_reuse_miss_corrupt",
                            1,
                        );
                    }
                }
                Some(lookup)
            }
            Err(_) => {
                nia_timing::emit_counter(
                    "frontend.extension_trait_solving_facts_reuse_miss_read_error",
                    1,
                );
                None
            }
        }
    } else {
        None
    };
    let cached = if db.context().verify_frontend_cache {
        cached
    } else {
        match cached {
            Some(crate::signature_cache::ExtensionTraitSolvingFactsLookup::Hit(cached)) => {
                return ExtensionTraitSolvingModuleFactsQueryValue {
                    trait_impls: cached.trait_impls,
                    invalid_trait_impl_method_ids: cached.invalid_trait_impl_method_ids,
                };
            }
            cached => cached,
        }
    };
    let input = db.get(ExtensionSignatureModuleInputQuery(module_id));
    let modules = [input.module(&db.context().type_store)];
    let fresh = crate::signature_cache::CachedExtensionTraitSolvingFacts {
        trait_impls: nia_program_signatures::collect_valid_program_trait_impls(&modules),
        invalid_trait_impl_method_ids:
            nia_program_signatures::collect_invalid_trait_impl_method_ids(&modules),
    };
    let cacheable = input.lowering.diagnostics.is_empty()
        && input.lowering.const_exprs.is_empty()
        && input.lowering.const_expr_summaries.is_empty()
        && input.signatures.diagnostics.is_empty()
        && input.function_signatures.diagnostics.is_empty()
        && input.type_signatures.diagnostics.is_empty()
        && input.normalization.diagnostics.is_empty();
    nia_timing::emit_counter(
        if cacheable {
            "frontend.extension_trait_solving_facts_cacheable"
        } else {
            "frontend.extension_trait_solving_facts_uncacheable"
        },
        1,
    );
    if let Some(cache) = &db.context().signature_cache
        && let Some((program_sources, source, namespace, key)) = cache_input
    {
        let replace = matches!(
            &cached,
            Some(crate::signature_cache::ExtensionTraitSolvingFactsLookup::Hit(cached))
                if cached.as_ref() != &fresh
        );
        if replace {
            cache.remove_extension_trait_solving_facts(key);
        }
        if cacheable {
            let _ = cache.publish_extension_trait_solving_facts(
                crate::signature_cache::ExtensionTraitSolvingFactsIdentity {
                    key,
                    namespace,
                    module: &source.module,
                    program_sources: program_sources.fingerprint,
                    source_len: source.len,
                },
                &fresh,
                &program_sources.path_by_module,
                &symbols,
                db.context().type_store(),
                replace,
            );
        }
    }
    ExtensionTraitSolvingModuleFactsQueryValue {
        trait_impls: fresh.trait_impls,
        invalid_trait_impl_method_ids: fresh.invalid_trait_impl_method_ids,
    }
}

pub(super) fn provide_extension_trait_impls_for_trait(
    db: &QueryDb<CompilerContext>,
    trait_id: nia_ty::TraitId,
) -> ExtensionTraitImplsForTraitValue {
    time_provider(
        db.context().timings(),
        "extension_trait_impls_for_trait",
        || {
            let candidate_modules = extension_trait_impl_candidate_modules(db, trait_id);
            let mut trait_impls = Vec::new();
            for facts in db.get_many(
                candidate_modules
                    .into_iter()
                    .map(ExtensionTraitSolvingModuleFactsQuery),
            ) {
                trait_impls.extend(
                    facts
                        .trait_impls
                        .iter()
                        .filter(|impl_signature| impl_signature.trait_id == trait_id)
                        .cloned(),
                );
            }
            ExtensionTraitImplsForTraitQueryValue { trait_impls }
        },
    )
}

fn extension_trait_impl_candidate_modules(
    db: &QueryDb<CompilerContext>,
    trait_id: nia_ty::TraitId,
) -> Vec<ModuleId> {
    let Some(name) = trait_id_index_name(db, trait_id) else {
        return Vec::new();
    };
    db.get(ExtensionProviderDiscoveryIndexQuery)
        .trait_impl_candidates_by_name
        .get(&name)
        .cloned()
        .unwrap_or_default()
}

fn trait_id_index_name(
    db: &QueryDb<CompilerContext>,
    trait_id: nia_ty::TraitId,
) -> Option<SymbolId> {
    match trait_id {
        nia_ty::TraitId::Builtin(trait_id) => Some(trait_id.symbol_id()),
        nia_ty::TraitId::Source(def_id) => db
            .get(ModuleDefsQuery(def_id.module_id))
            .defs
            .get(def_id.def_id)
            .map(|def| def.name),
    }
}

pub(super) fn provide_extension_provider_module_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ExtensionProviderModuleFactsValue {
    time_module_provider(db, "extension_provider_module_facts", module_id, || {
        if !*db.get(ExtensionProviderModuleEligibilityQuery(module_id)) {
            return ExtensionProviderModuleFactsQueryValue {
                methods: nia_defs::ExtensionMethods::default(),
                associated_values: nia_defs::ExtensionAssociatedValues::default(),
                associated_value_diagnostics: Vec::new(),
                nominal_providers: Vec::new(),
            };
        }

        let defs = db.get(ModuleDefsQuery(module_id));
        let lowering = db.get(SignatureTypeLoweringQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let signatures = db.get(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let normalization = db.get(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let module = ExtensionMethodIndexModuleInput {
            module_id,
            type_store: &db.context().type_store,
            defs: &defs,
            lowering: &lowering,
            signatures: &signatures,
            normalization: &normalization,
        };
        let module_defs = SharedProgramDefsResolver::new(db);
        let methods = collect_extension_method_index_for_module(&module, &module_defs);
        let (associated_values, associated_value_diagnostics) =
            collect_extension_associated_value_index_for_module(&module);
        let nominal_providers =
            collect_nominal_extension_providers_for_module(&module, &module_defs);
        ExtensionProviderModuleFactsQueryValue {
            methods,
            associated_values,
            associated_value_diagnostics,
            nominal_providers,
        }
    })
}

pub(super) fn provide_extension_provider_validation_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ExtensionProviderValidationFactsValue {
    time_module_provider(db, "extension_provider_validation_facts", module_id, || {
        let program_sources = db.get(FrontendProgramSourcesQuery);
        let cache_input = program_sources
            .as_ref()
            .as_ref()
            .and_then(|program_sources| {
                let source = program_sources.by_module.get(&module_id)?;
                let namespace = crate::FrontendCacheNamespace::new(
                    &db.context().loader_facts.target(),
                    db.context().loader_facts.runtime(),
                );
                let key = crate::FrontendExtensionValidationDiagnosticsCacheKey::new(
                    namespace,
                    &source.module,
                    program_sources.fingerprint,
                );
                Some((program_sources, source, namespace, key))
            });
        let cached = if let Some(cache) = db.context().signature_cache.as_ref()
            && let Some((program_sources, source, namespace, key)) = cache_input
        {
            match cache.load_extension_validation_diagnostics(
                crate::signature_cache::ExtensionValidationDiagnosticsIdentity {
                    key,
                    namespace,
                    module: &source.module,
                    program_sources: program_sources.fingerprint,
                    source_len: source.len,
                },
            ) {
                Ok(lookup) => {
                    match lookup {
                        crate::signature_cache::ExtensionValidationDiagnosticsLookup::Hit(_) => {
                            nia_timing::emit_counter(
                                "frontend.extension_validation_diagnostics_reuse_hits",
                                1,
                            );
                        }
                        crate::signature_cache::ExtensionValidationDiagnosticsLookup::NotFound => {
                            nia_timing::emit_counter(
                                "frontend.extension_validation_diagnostics_reuse_miss_not_found",
                                1,
                            );
                        }
                        crate::signature_cache::ExtensionValidationDiagnosticsLookup::Corrupt => {
                            nia_timing::emit_counter(
                                "frontend.extension_validation_diagnostics_reuse_miss_corrupt",
                                1,
                            );
                        }
                    }
                    Some(lookup)
                }
                Err(_) => {
                    nia_timing::emit_counter(
                        "frontend.extension_validation_diagnostics_reuse_miss_read_error",
                        1,
                    );
                    None
                }
            }
        } else {
            None
        };
        let cached = if db.context().verify_frontend_cache {
            cached
        } else {
            match cached {
                Some(crate::signature_cache::ExtensionValidationDiagnosticsLookup::Hit(cached)) => {
                    return ExtensionProviderValidationFactsQueryValue {
                        diagnostics: cached,
                    };
                }
                cached => cached,
            }
        };
        let diagnostics = if !*db.get(ExtensionProviderModuleEligibilityQuery(module_id)) {
            Vec::new()
        } else {
            let input = db.get(ExtensionSignatureModuleInputQuery(module_id));
            let trait_index = db.get(ExtensionTraitSignatureIndexQuery);
            let trait_impls_for_trait = |trait_id| {
                db.get(ExtensionTraitImplsForTraitQuery(trait_id))
                    .trait_impls
                    .clone()
            };
            let symbols = db.context().symbols();
            collect_extension_method_diagnostics_for_module(
                &input.module(&db.context().type_store),
                ExtensionMethodValidationInput {
                    type_store: &db.context().type_store,
                    trait_defs: &trait_index.trait_defs,
                    trait_signatures: &trait_index.trait_signatures,
                    trait_impls_for_trait: &trait_impls_for_trait,
                    symbols: &symbols,
                },
            )
        };
        if let Some(cache) = &db.context().signature_cache
            && let Some((program_sources, source, namespace, key)) = cache_input
        {
            let replace = matches!(
                &cached,
                Some(crate::signature_cache::ExtensionValidationDiagnosticsLookup::Hit(cached))
                    if cached != &diagnostics
            );
            if replace {
                cache.remove_extension_validation_diagnostics(key);
            }
            let published = cache.publish_extension_validation_diagnostics(
                crate::signature_cache::ExtensionValidationDiagnosticsIdentity {
                    key,
                    namespace,
                    module: &source.module,
                    program_sources: program_sources.fingerprint,
                    source_len: source.len,
                },
                &diagnostics,
                replace,
            );
            nia_timing::emit_counter(
                if published.is_ok() {
                    "frontend.extension_validation_diagnostics_cacheable"
                } else {
                    "frontend.extension_validation_diagnostics_uncacheable"
                },
                1,
            );
        }
        ExtensionProviderValidationFactsQueryValue { diagnostics }
    })
}

fn extension_provider_module_facts(
    db: &QueryDb<CompilerContext>,
) -> Vec<Arc<ExtensionProviderModuleFactsValue>> {
    let module_ids = resolve_stable_module_sequence(db, &db.get(ExtensionProviderModuleIdsQuery));
    db.get_many(
        module_ids
            .into_iter()
            .map(ExtensionProviderModuleFactsQuery),
    )
}

pub(super) fn provide_extension_provider_nominal_module_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ExtensionProviderNominalModuleFactsValue {
    time_module_provider(
        db,
        "extension_provider_nominal_module_facts",
        module_id,
        || {
            if !*db.get(ExtensionProviderModuleEligibilityQuery(module_id)) {
                return ExtensionProviderNominalModuleFactsQueryValue {
                    nominal_providers: Vec::new(),
                };
            }

            let defs = db.get(ModuleDefsQuery(module_id));
            let lowering = db.get(SignatureTypeLoweringQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Traits,
            ));
            let signatures = db.get(SignatureItemSignaturesQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Traits,
            ));
            let normalization = db.get(SignatureTypeNormalizationQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Traits,
            ));
            let module = ExtensionMethodIndexModuleInput {
                module_id,
                type_store: &db.context().type_store,
                defs: &defs,
                lowering: &lowering,
                signatures: &signatures,
                normalization: &normalization,
            };
            let module_defs = SharedProgramDefsResolver::new(db);
            let nominal_providers =
                collect_nominal_extension_providers_for_module(&module, &module_defs);
            ExtensionProviderNominalModuleFactsQueryValue { nominal_providers }
        },
    )
}

pub(super) fn provide_extension_provider_nominal_candidate_modules(
    db: &QueryDb<CompilerContext>,
    names: ExtensionProviderNominalTargetNames,
) -> ExtensionProviderNominalCandidateModulesValue {
    time_provider(
        db.context().timings(),
        "extension_provider_nominal_candidate_modules",
        || {
            let discovery = db.get(ExtensionProviderDiscoveryIndexQuery);
            let mut modules = Vec::new();
            for name in names.0 {
                if let Some(candidates) = discovery.nominal_candidates_by_name.get(&name) {
                    modules.extend(candidates.iter().copied());
                }
            }
            modules.sort();
            modules.dedup();
            ExtensionProviderNominalCandidateModulesQueryValue { modules }
        },
    )
}

pub(super) fn provide_extension_provider_nominal_modules_for_targets(
    db: &QueryDb<CompilerContext>,
    targets: ExtensionProviderNominalTargets,
    accessing_module: ModuleId,
) -> ExtensionProviderNominalModulesForTargetsValue {
    time_provider(
        db.context().timings(),
        "extension_provider_nominal_modules_for_targets",
        || {
            let graph = db.get(ModuleGraphQuery);
            let index_names = extension_provider_nominal_target_names_for_targets(db, &targets);
            let candidate_modules = db
                .get(ExtensionProviderNominalCandidateModulesQuery(
                    ExtensionProviderNominalTargetNames::new(index_names),
                ))
                .modules
                .clone();
            let mut modules = Vec::new();
            for facts in db.get_many(
                candidate_modules
                    .into_iter()
                    .map(ExtensionProviderNominalModuleFactsQuery),
            ) {
                modules.extend(
                    facts
                        .nominal_providers
                        .iter()
                        .copied()
                        .filter(|provider| {
                            targets.as_slice().binary_search(&provider.target).is_ok()
                                && nia_imports::visibility_allows(
                                    provider.visibility,
                                    &graph,
                                    provider.module_id,
                                    accessing_module,
                                )
                        })
                        .map(|provider| provider.module_id),
                );
            }
            modules.sort();
            modules.dedup();
            ExtensionProviderNominalModulesForTargetsQueryValue { modules }
        },
    )
}

fn extension_provider_nominal_target_names_for_targets(
    db: &QueryDb<CompilerContext>,
    targets: &ExtensionProviderNominalTargets,
) -> Vec<SymbolId> {
    let type_exposures = db.get(TypeExposureIndexQuery);
    let mut names = Vec::new();

    for target in targets.as_slice().iter().copied() {
        names.extend(type_exposures.names_for(target).iter().cloned());
    }

    names.sort();
    names.dedup();
    names
}

pub(super) fn provide_extension_method_index(
    db: &QueryDb<CompilerContext>,
) -> ExtensionMethodIndexValue {
    time_provider(db.context().timings(), "extension_method_index", || {
        let mut methods = nia_defs::ExtensionMethods::default();
        for facts in extension_provider_module_facts(db) {
            methods.extend(facts.methods.clone());
        }
        ExtensionMethodIndexQueryValue { methods }
    })
}

pub(super) fn provide_extension_methods_named(
    db: &QueryDb<CompilerContext>,
    name: SymbolId,
) -> ExtensionMethodsNamedValue {
    time_provider(db.context().timings(), "extension_methods_named", || {
        let discovery = db.get(ExtensionProviderDiscoveryIndexQuery);
        let mut methods = Vec::new();
        if let Some(candidate_modules) = discovery.method_candidates_by_name.get(&name) {
            for facts in db.get_many(
                candidate_modules
                    .iter()
                    .copied()
                    .map(ExtensionProviderModuleFactsQuery),
            ) {
                methods.extend(facts.methods.methods_named(&name).cloned());
            }
        }
        ExtensionMethodsNamedQueryValue { methods }
    })
}

pub(super) fn provide_extension_method_by_id(
    db: &QueryDb<CompilerContext>,
    def_id: GlobalDefId,
) -> ExtensionMethodByIdValue {
    time_provider(db.context().timings(), "extension_method_by_id", || {
        let method = db
            .get(ExtensionProviderModuleFactsQuery(def_id.module_id))
            .methods
            .method_by_id(def_id)
            .cloned();
        ExtensionMethodByIdQueryValue { method }
    })
}

pub(super) fn provide_extension_trait_signature_index(
    db: &QueryDb<CompilerContext>,
) -> ExtensionTraitSignatureIndexValue {
    time_provider(
        db.context().timings(),
        "extension_trait_signature_index",
        || {
            let facts = program_signature_facts(db, nia_item_tree::SignatureItemSet::Traits);
            let mut trait_defs = HashSet::new();
            let mut trait_signatures = HashMap::new();
            for module_facts in facts {
                trait_defs.extend(module_facts.trait_defs.iter().copied());
                trait_signatures.extend(
                    module_facts
                        .traits
                        .iter()
                        .map(|(def_id, signature)| (*def_id, signature.clone())),
                );
            }
            ExtensionTraitSignatureIndex {
                trait_defs,
                trait_signatures,
            }
        },
    )
}

fn visible_provider_modules_for_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> Vec<ModuleId> {
    visible_modules_for_module(
        db,
        module_id,
        nia_program_signatures::visible_extension_provider_modules,
    )
}

fn visible_trait_impl_modules_for_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> Vec<ModuleId> {
    visible_modules_for_module(
        db,
        module_id,
        nia_program_signatures::visible_trait_impl_modules,
    )
}

fn visible_modules_for_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    compute: fn(nia_program_signatures::VisibleExtensionProviderModulesInput<'_>) -> Vec<ModuleId>,
) -> Vec<ModuleId> {
    let graph = QueryModuleGraphLookup::new(db);
    let defs = SharedProgramDefsResolver::new(db);
    let using_scopes = |module_id| Some(db.get(ModuleUsingScopeQuery(module_id)));
    let using_scope = db.get(ModuleUsingScopeQuery(module_id));
    let extension_method_normalization = |module_id| {
        Some(db.get(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        )))
    };
    let type_alias = |def_id| {
        db.get(ProgramTypeAliasSignatureQuery(def_id))
            .as_ref()
            .clone()
    };
    let nominal_extension_providers = |target_def_ids: &[GlobalDefId]| {
        db.get(ExtensionProviderNominalModulesForTargetsQuery(
            ExtensionProviderNominalTargets::new(target_def_ids.to_vec()),
            module_id,
        ))
        .modules
        .clone()
    };
    let provider_modules = compute(
        nia_program_signatures::VisibleExtensionProviderModulesInput {
            module_id,
            type_store: &db.context().type_store,
            graph: &graph,
            using_scope: &using_scope,
            using_scopes: &using_scopes,
            defs: &defs,
            normalizations: &extension_method_normalization,
            visible_type_signatures: VisibleTypeSignatures {
                type_alias: &type_alias,
            },
            nominal_extension_providers: &nominal_extension_providers,
        },
    );
    let mut visible_modules = Vec::with_capacity(provider_modules.len() + 1);
    visible_modules.push(module_id);
    visible_modules.extend(provider_modules);
    visible_modules.sort();
    visible_modules.dedup();
    visible_modules
}

pub(super) fn provide_visible_extensions(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> VisibleExtensionsValue {
    let graph = QueryModuleGraphLookup::new(db);
    let defs = SharedProgramDefsResolver::new(db);
    let public_surfaces = QueryPublicSurfaceLookup::new(db);
    let using_scopes = |module_id| Some(db.get(ModuleUsingScopeQuery(module_id)));
    let using_scope = db.get(ModuleUsingScopeQuery(module_id));
    let extension_method_normalization = |module_id| {
        Some(db.get(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        )))
    };
    let type_alias = |def_id| {
        db.get(ProgramTypeAliasSignatureQuery(def_id))
            .as_ref()
            .clone()
    };
    let nominal_extension_providers = |target_def_ids: &[GlobalDefId]| {
        db.get(ExtensionProviderNominalModulesForTargetsQuery(
            ExtensionProviderNominalTargets::new(target_def_ids.to_vec()),
            module_id,
        ))
        .modules
        .clone()
    };
    let visible_modules = visible_provider_modules_for_module(db, module_id);
    let mut extension_methods = nia_defs::ExtensionMethods::default();
    let mut associated_values = nia_defs::ExtensionAssociatedValues::default();
    for provider_module in visible_modules.iter().copied() {
        let facts = db.get(ExtensionProviderModuleFactsQuery(provider_module));
        extension_methods.extend(facts.methods.clone());
        associated_values.extend(facts.associated_values.clone());
    }
    visible_extensions_for_module(VisibleExtensionsInput {
        module_id,
        type_store: &db.context().type_store,
        graph: &graph,
        using_scope: &using_scope,
        using_scopes: &using_scopes,
        public_surfaces: &public_surfaces,
        defs: &defs,
        normalizations: &extension_method_normalization,
        visible_type_signatures: VisibleTypeSignatures {
            type_alias: &type_alias,
        },
        extensions: &extension_methods,
        associated_values: &associated_values,
        trait_impls: &[],
        nominal_extension_providers: &nominal_extension_providers,
        visible_modules: Some(visible_modules.as_slice()),
    })
}

pub(super) fn provide_visible_trait_impls(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> VisibleTraitImplsValue {
    let graph = QueryModuleGraphLookup::new(db);
    let defs = SharedProgramDefsResolver::new(db);
    let public_surfaces = QueryPublicSurfaceLookup::new(db);
    let using_scopes = |module_id| Some(db.get(ModuleUsingScopeQuery(module_id)));
    let using_scope = db.get(ModuleUsingScopeQuery(module_id));
    let extension_method_normalization = |module_id| {
        Some(db.get(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        )))
    };
    let type_alias = |def_id| {
        db.get(ProgramTypeAliasSignatureQuery(def_id))
            .as_ref()
            .clone()
    };
    let nominal_extension_providers = |target_def_ids: &[GlobalDefId]| {
        db.get(ExtensionProviderNominalModulesForTargetsQuery(
            ExtensionProviderNominalTargets::new(target_def_ids.to_vec()),
            module_id,
        ))
        .modules
        .clone()
    };
    let visible_modules = visible_trait_impl_modules_for_module(db, module_id);
    let mut trait_impls = Vec::new();
    for provider_module in visible_modules.iter().copied() {
        trait_impls.extend(
            db.get(ExtensionTraitSolvingModuleFactsQuery(provider_module))
                .trait_impls
                .iter()
                .cloned(),
        );
    }
    visible_trait_impls_for_module(VisibleExtensionsInput {
        module_id,
        type_store: &db.context().type_store,
        graph: &graph,
        using_scope: &using_scope,
        using_scopes: &using_scopes,
        public_surfaces: &public_surfaces,
        defs: &defs,
        normalizations: &extension_method_normalization,
        visible_type_signatures: VisibleTypeSignatures {
            type_alias: &type_alias,
        },
        extensions: &nia_defs::ExtensionMethods::default(),
        associated_values: &nia_defs::ExtensionAssociatedValues::default(),
        trait_impls: trait_impls.as_slice(),
        nominal_extension_providers: &nominal_extension_providers,
        visible_modules: Some(visible_modules.as_slice()),
    })
}
