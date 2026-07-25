// SPDX-License-Identifier: GPL-3.0-or-later
use super::program_signatures::program_signature_facts;
use super::*;
use nia_symbol::ToSymbolId;
use std::cell::RefCell;

struct SharedProgramDefsResolver<'a> {
    db: &'a QueryDb<CompilerContext>,
    cache: RefCell<HashMap<ModuleId, Option<Arc<DefCollection>>>>,
    failure: RefCell<Option<QueryError>>,
}

impl<'a> SharedProgramDefsResolver<'a> {
    fn new(db: &'a QueryDb<CompilerContext>) -> Self {
        Self {
            db,
            cache: RefCell::new(HashMap::new()),
            failure: RefCell::new(None),
        }
    }

    fn take_failure(&self) -> Option<QueryError> {
        self.failure.borrow_mut().take()
    }
}

impl nia_program_signatures::ProgramDefsResolver for SharedProgramDefsResolver<'_> {
    fn defs(&self, module_id: ModuleId) -> Option<Arc<DefCollection>> {
        if let Some(defs) = self.cache.borrow().get(&module_id) {
            return defs.clone();
        }
        let defs =
            capture_query_failure(&self.failure, self.db.try_get(ModuleDefsQuery(module_id)));
        self.cache.borrow_mut().insert(module_id, defs.clone());
        defs
    }
}

pub(super) fn provide_extension_provider_summary(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<nia_provider_summary::ProviderSummary> {
    db.context().module_provider_summary(db, module_id)
}

pub(super) fn provide_extension_provider_discovery_index(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<ExtensionProviderDiscoveryIndexValue> {
    let timings = db.context().timings();
    let parse_ok_modules = db.try_get(ParseOkModuleIdsQuery)?;
    let parse_ok_modules = resolve_stable_module_sequence(db, &parse_ok_modules);
    let mut trait_modules = Vec::new();
    for module_id in parse_ok_modules {
        if *db.try_get(ExtensionProviderModuleEligibilityQuery(module_id))? {
            trait_modules.push(module_id);
        }
    }
    time_provider(timings, "extension_provider_discovery_index", || {
        let mut provider_modules = Vec::new();
        let mut nominal_candidates_by_name: HashMap<SymbolId, Vec<ModuleId>> = HashMap::new();
        let mut method_candidates_by_name: HashMap<SymbolId, Vec<ModuleId>> = HashMap::new();
        let mut trait_impl_candidates_by_name: HashMap<SymbolId, Vec<ModuleId>> = HashMap::new();
        for module_id in trait_modules {
            let summary = db.try_get(ExtensionProviderSummaryQuery(module_id))?;
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
        Ok(ExtensionProviderDiscoveryIndexQueryValue {
            provider_modules,
            nominal_candidates_by_name,
            method_candidates_by_name,
            trait_impl_candidates_by_name,
        })
    })
}

pub(super) fn provide_extension_provider_module_ids(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<StableModuleSequence> {
    let parse_ok_modules = db.try_get(ParseOkModuleIdsQuery)?;
    let parse_ok_modules =
        resolve_stable_module_sequence_from_current_inputs(db, &parse_ok_modules);
    let mut module_ids = Vec::new();
    for module_id in parse_ok_modules {
        if *db.try_get(ExtensionProviderModuleEligibilityQuery(module_id))? {
            module_ids.push(module_id);
        }
    }
    Ok(stable_module_sequence(db, module_ids))
}

pub(super) fn provide_extension_provider_module_eligibility(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<bool> {
    Ok(db
        .try_get(ExtensionProviderSummaryQuery(module_id))?
        .has_providers())
}

pub(super) fn provide_extension_signature_module_input(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ExtensionSignatureModuleInputValue> {
    Ok(ExtensionSignatureModuleInputQueryValue {
        module_id,
        defs: db.try_get(ModuleDefsQuery(module_id))?,
        lowering: db.try_get(SignatureTypeLoweringQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ))?,
        signatures: db.try_get(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ))?,
        function_signatures: db.try_get(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::ExtensionFunctions,
        ))?,
        type_signatures: db.try_get(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))?,
        normalization: db.try_get(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ))?,
    })
}

pub(super) fn provide_extension_trait_solving_module_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ExtensionTraitSolvingModuleFactsValue> {
    let program_sources = db.try_get(FrontendProgramSourcesQuery)?;
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
                return Ok(ExtensionTraitSolvingModuleFactsQueryValue {
                    trait_impls: cached.trait_impls,
                    invalid_trait_impl_method_ids: cached.invalid_trait_impl_method_ids,
                });
            }
            cached => cached,
        }
    };
    let input = db.try_get(ExtensionSignatureModuleInputQuery(module_id))?;
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
    Ok(ExtensionTraitSolvingModuleFactsQueryValue {
        trait_impls: fresh.trait_impls,
        invalid_trait_impl_method_ids: fresh.invalid_trait_impl_method_ids,
    })
}

pub(super) fn provide_extension_trait_impls_for_trait(
    db: &QueryDb<CompilerContext>,
    trait_id: nia_ty::TraitId,
) -> QueryResult<ExtensionTraitImplsForTraitValue> {
    time_provider(
        db.context().timings(),
        "extension_trait_impls_for_trait",
        || {
            let candidate_modules = extension_trait_impl_candidate_modules(db, trait_id)?;
            let mut trait_impls = Vec::new();
            for module_id in candidate_modules {
                let facts = db.try_get(ExtensionTraitSolvingModuleFactsQuery(module_id))?;
                trait_impls.extend(
                    facts
                        .trait_impls
                        .iter()
                        .filter(|impl_signature| impl_signature.trait_id == trait_id)
                        .cloned(),
                );
            }
            Ok(ExtensionTraitImplsForTraitQueryValue { trait_impls })
        },
    )
}

fn extension_trait_impl_candidate_modules(
    db: &QueryDb<CompilerContext>,
    trait_id: nia_ty::TraitId,
) -> QueryResult<Vec<ModuleId>> {
    let Some(name) = trait_id_index_name(db, trait_id)? else {
        return Ok(Vec::new());
    };
    Ok(db
        .try_get(ExtensionProviderDiscoveryIndexQuery)?
        .trait_impl_candidates_by_name
        .get(&name)
        .cloned()
        .unwrap_or_default())
}

fn trait_id_index_name(
    db: &QueryDb<CompilerContext>,
    trait_id: nia_ty::TraitId,
) -> QueryResult<Option<SymbolId>> {
    Ok(match trait_id {
        nia_ty::TraitId::Builtin(trait_id) => Some(trait_id.symbol_id()),
        nia_ty::TraitId::Source(def_id) => db
            .try_get(ModuleDefsQuery(def_id.module_id))?
            .defs
            .get(def_id.def_id)
            .map(|def| def.name),
    })
}

pub(super) fn provide_extension_provider_module_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ExtensionProviderModuleFactsValue> {
    time_module_provider(db, "extension_provider_module_facts", module_id, || {
        if !*db.try_get(ExtensionProviderModuleEligibilityQuery(module_id))? {
            return Ok(ExtensionProviderModuleFactsQueryValue {
                methods: nia_defs::ExtensionMethods::default(),
                associated_values: nia_defs::ExtensionAssociatedValues::default(),
                associated_value_diagnostics: Vec::new(),
                nominal_providers: Vec::new(),
            });
        }

        let defs = db.try_get(ModuleDefsQuery(module_id))?;
        let lowering = db.try_get(SignatureTypeLoweringQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ))?;
        let signatures = db.try_get(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ))?;
        let normalization = db.try_get(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ))?;
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
        if let Some(error) = module_defs.take_failure() {
            return Err(error);
        }
        Ok(ExtensionProviderModuleFactsQueryValue {
            methods,
            associated_values,
            associated_value_diagnostics,
            nominal_providers,
        })
    })
}

pub(super) fn provide_extension_provider_validation_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ExtensionProviderValidationFactsValue> {
    time_module_provider(db, "extension_provider_validation_facts", module_id, || {
        let program_sources = db.try_get(FrontendProgramSourcesQuery)?;
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
                    return Ok(ExtensionProviderValidationFactsQueryValue {
                        diagnostics: cached,
                    });
                }
                cached => cached,
            }
        };
        let query_failure = RefCell::new(None);
        let diagnostics = if !*db.try_get(ExtensionProviderModuleEligibilityQuery(module_id))? {
            Vec::new()
        } else {
            let input = db.try_get(ExtensionSignatureModuleInputQuery(module_id))?;
            let trait_index = db.try_get(ExtensionTraitSignatureIndexQuery)?;
            let trait_impls_for_trait = |trait_id| {
                capture_query_failure(
                    &query_failure,
                    db.try_get(ExtensionTraitImplsForTraitQuery(trait_id)),
                )
                .map(|facts| facts.trait_impls.clone())
                .unwrap_or_default()
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
        if let Some(error) = query_failure.into_inner() {
            return Err(error);
        }
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
        Ok(ExtensionProviderValidationFactsQueryValue { diagnostics })
    })
}

fn extension_provider_module_facts(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<Vec<Arc<ExtensionProviderModuleFactsValue>>> {
    let module_sequence = db.try_get(ExtensionProviderModuleIdsQuery)?;
    let module_ids = resolve_stable_module_sequence(db, &module_sequence);
    module_ids
        .into_iter()
        .map(|module_id| db.try_get(ExtensionProviderModuleFactsQuery(module_id)))
        .collect()
}

pub(super) fn provide_extension_provider_nominal_module_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ExtensionProviderNominalModuleFactsValue> {
    time_module_provider(
        db,
        "extension_provider_nominal_module_facts",
        module_id,
        || {
            if !*db.try_get(ExtensionProviderModuleEligibilityQuery(module_id))? {
                return Ok(ExtensionProviderNominalModuleFactsQueryValue {
                    nominal_providers: Vec::new(),
                });
            }

            let defs = db.try_get(ModuleDefsQuery(module_id))?;
            let lowering = db.try_get(SignatureTypeLoweringQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Traits,
            ))?;
            let signatures = db.try_get(SignatureItemSignaturesQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Traits,
            ))?;
            let normalization = db.try_get(SignatureTypeNormalizationQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Traits,
            ))?;
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
            if let Some(error) = module_defs.take_failure() {
                return Err(error);
            }
            Ok(ExtensionProviderNominalModuleFactsQueryValue { nominal_providers })
        },
    )
}

pub(super) fn provide_extension_provider_nominal_candidate_modules(
    db: &QueryDb<CompilerContext>,
    names: ExtensionProviderNominalTargetNames,
) -> QueryResult<ExtensionProviderNominalCandidateModulesValue> {
    time_provider(
        db.context().timings(),
        "extension_provider_nominal_candidate_modules",
        || {
            let discovery = db.try_get(ExtensionProviderDiscoveryIndexQuery)?;
            let mut modules = Vec::new();
            for name in names.0 {
                if let Some(candidates) = discovery.nominal_candidates_by_name.get(&name) {
                    modules.extend(candidates.iter().copied());
                }
            }
            modules.sort();
            modules.dedup();
            Ok(ExtensionProviderNominalCandidateModulesQueryValue { modules })
        },
    )
}

pub(super) fn provide_extension_provider_nominal_modules_for_targets(
    db: &QueryDb<CompilerContext>,
    targets: ExtensionProviderNominalTargets,
    accessing_module: ModuleId,
) -> QueryResult<ExtensionProviderNominalModulesForTargetsValue> {
    time_provider(
        db.context().timings(),
        "extension_provider_nominal_modules_for_targets",
        || {
            let graph = db.try_get(ModuleGraphQuery)?;
            let index_names = extension_provider_nominal_target_names_for_targets(db, &targets)?;
            let candidate_modules = db
                .try_get(ExtensionProviderNominalCandidateModulesQuery(
                    ExtensionProviderNominalTargetNames::new(index_names),
                ))?
                .modules
                .clone();
            let mut modules = Vec::new();
            for module_id in candidate_modules {
                let facts = db.try_get(ExtensionProviderNominalModuleFactsQuery(module_id))?;
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
            Ok(ExtensionProviderNominalModulesForTargetsQueryValue { modules })
        },
    )
}

fn extension_provider_nominal_target_names_for_targets(
    db: &QueryDb<CompilerContext>,
    targets: &ExtensionProviderNominalTargets,
) -> QueryResult<Vec<SymbolId>> {
    let type_exposures = db.try_get(TypeExposureIndexQuery)?;
    let mut names = Vec::new();

    for target in targets.as_slice().iter().copied() {
        names.extend(type_exposures.names_for(target).iter().cloned());
    }

    names.sort();
    names.dedup();
    Ok(names)
}

pub(super) fn provide_extension_method_index(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<ExtensionMethodIndexValue> {
    time_provider(db.context().timings(), "extension_method_index", || {
        let mut methods = nia_defs::ExtensionMethods::default();
        for facts in extension_provider_module_facts(db)? {
            methods.extend(facts.methods.clone());
        }
        Ok(ExtensionMethodIndexQueryValue { methods })
    })
}

pub(super) fn provide_extension_methods_named(
    db: &QueryDb<CompilerContext>,
    name: SymbolId,
) -> QueryResult<ExtensionMethodsNamedValue> {
    time_provider(db.context().timings(), "extension_methods_named", || {
        let discovery = db.try_get(ExtensionProviderDiscoveryIndexQuery)?;
        let mut methods = Vec::new();
        if let Some(candidate_modules) = discovery.method_candidates_by_name.get(&name) {
            for module_id in candidate_modules.iter().copied() {
                let facts = db.try_get(ExtensionProviderModuleFactsQuery(module_id))?;
                methods.extend(facts.methods.methods_named(&name).cloned());
            }
        }
        Ok(ExtensionMethodsNamedQueryValue { methods })
    })
}

pub(super) fn provide_extension_method_by_id(
    db: &QueryDb<CompilerContext>,
    def_id: GlobalDefId,
) -> QueryResult<ExtensionMethodByIdValue> {
    time_provider(db.context().timings(), "extension_method_by_id", || {
        let method = db
            .try_get(ExtensionProviderModuleFactsQuery(def_id.module_id))?
            .methods
            .method_by_id(def_id)
            .cloned();
        Ok(ExtensionMethodByIdQueryValue { method })
    })
}

pub(super) fn provide_extension_trait_signature_index(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<ExtensionTraitSignatureIndexValue> {
    time_provider(
        db.context().timings(),
        "extension_trait_signature_index",
        || {
            let facts = program_signature_facts(db, nia_item_tree::SignatureItemSet::Traits)?;
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
            Ok(ExtensionTraitSignatureIndex {
                trait_defs,
                trait_signatures,
            })
        },
    )
}

fn visible_provider_modules_for_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<Vec<ModuleId>> {
    visible_modules_for_module(
        db,
        module_id,
        nia_program_signatures::visible_extension_provider_modules,
    )
}

fn visible_trait_impl_modules_for_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<Vec<ModuleId>> {
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
) -> QueryResult<Vec<ModuleId>> {
    let graph = QueryModuleGraphLookup::new(db)?;
    let defs = SharedProgramDefsResolver::new(db);
    let query_failure = RefCell::new(None);
    let using_scopes = |module_id| {
        capture_query_failure(&query_failure, db.try_get(ModuleUsingScopeQuery(module_id)))
    };
    let using_scope = db.try_get(ModuleUsingScopeQuery(module_id))?;
    let extension_method_normalization = |module_id| {
        capture_query_failure(
            &query_failure,
            db.try_get(SignatureTypeNormalizationQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Traits,
            )),
        )
    };
    let type_alias = |def_id| {
        capture_query_failure(
            &query_failure,
            db.try_get(ProgramTypeAliasSignatureQuery(def_id)),
        )
        .and_then(|signature| signature.as_ref().clone())
    };
    let nominal_extension_providers = |target_def_ids: &[GlobalDefId]| {
        capture_query_failure(
            &query_failure,
            db.try_get(ExtensionProviderNominalModulesForTargetsQuery(
                ExtensionProviderNominalTargets::new(target_def_ids.to_vec()),
                module_id,
            )),
        )
        .map(|providers| providers.modules.clone())
        .unwrap_or_default()
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
    if let Some(error) = query_failure
        .into_inner()
        .or_else(|| graph.take_failure())
        .or_else(|| defs.take_failure())
    {
        Err(error)
    } else {
        Ok(visible_modules)
    }
}

pub(super) fn provide_visible_extensions(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<VisibleExtensionsValue> {
    let graph = QueryModuleGraphLookup::new(db)?;
    let defs = SharedProgramDefsResolver::new(db);
    let public_surfaces = QueryPublicSurfaceLookup::new(db);
    let query_failure = RefCell::new(None);
    let using_scopes = |module_id| {
        capture_query_failure(&query_failure, db.try_get(ModuleUsingScopeQuery(module_id)))
    };
    let using_scope = db.try_get(ModuleUsingScopeQuery(module_id))?;
    let extension_method_normalization = |module_id| {
        capture_query_failure(
            &query_failure,
            db.try_get(SignatureTypeNormalizationQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Traits,
            )),
        )
    };
    let type_alias = |def_id| {
        capture_query_failure(
            &query_failure,
            db.try_get(ProgramTypeAliasSignatureQuery(def_id)),
        )
        .and_then(|signature| signature.as_ref().clone())
    };
    let nominal_extension_providers = |target_def_ids: &[GlobalDefId]| {
        capture_query_failure(
            &query_failure,
            db.try_get(ExtensionProviderNominalModulesForTargetsQuery(
                ExtensionProviderNominalTargets::new(target_def_ids.to_vec()),
                module_id,
            )),
        )
        .map(|providers| providers.modules.clone())
        .unwrap_or_default()
    };
    let visible_modules = visible_provider_modules_for_module(db, module_id)?;
    let mut extension_methods = nia_defs::ExtensionMethods::default();
    let mut associated_values = nia_defs::ExtensionAssociatedValues::default();
    for provider_module in visible_modules.iter().copied() {
        let facts = db.try_get(ExtensionProviderModuleFactsQuery(provider_module))?;
        extension_methods.extend(facts.methods.clone());
        associated_values.extend(facts.associated_values.clone());
    }
    let visible_extensions = visible_extensions_for_module(VisibleExtensionsInput {
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
    });
    if let Some(error) = query_failure
        .into_inner()
        .or_else(|| graph.take_failure())
        .or_else(|| defs.take_failure())
        .or_else(|| public_surfaces.take_failure())
    {
        Err(error)
    } else {
        Ok(visible_extensions)
    }
}

pub(super) fn provide_visible_trait_impls(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<VisibleTraitImplsValue> {
    let graph = QueryModuleGraphLookup::new(db)?;
    let defs = SharedProgramDefsResolver::new(db);
    let public_surfaces = QueryPublicSurfaceLookup::new(db);
    let query_failure = RefCell::new(None);
    let using_scopes = |module_id| {
        capture_query_failure(&query_failure, db.try_get(ModuleUsingScopeQuery(module_id)))
    };
    let using_scope = db.try_get(ModuleUsingScopeQuery(module_id))?;
    let extension_method_normalization = |module_id| {
        capture_query_failure(
            &query_failure,
            db.try_get(SignatureTypeNormalizationQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Traits,
            )),
        )
    };
    let type_alias = |def_id| {
        capture_query_failure(
            &query_failure,
            db.try_get(ProgramTypeAliasSignatureQuery(def_id)),
        )
        .and_then(|signature| signature.as_ref().clone())
    };
    let nominal_extension_providers = |target_def_ids: &[GlobalDefId]| {
        capture_query_failure(
            &query_failure,
            db.try_get(ExtensionProviderNominalModulesForTargetsQuery(
                ExtensionProviderNominalTargets::new(target_def_ids.to_vec()),
                module_id,
            )),
        )
        .map(|providers| providers.modules.clone())
        .unwrap_or_default()
    };
    let visible_modules = visible_trait_impl_modules_for_module(db, module_id)?;
    let mut trait_impls = Vec::new();
    for provider_module in visible_modules.iter().copied() {
        trait_impls.extend(
            db.try_get(ExtensionTraitSolvingModuleFactsQuery(provider_module))?
                .trait_impls
                .iter()
                .cloned(),
        );
    }
    let visible_trait_impls = visible_trait_impls_for_module(VisibleExtensionsInput {
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
    });
    if let Some(error) = query_failure
        .into_inner()
        .or_else(|| graph.take_failure())
        .or_else(|| defs.take_failure())
        .or_else(|| public_surfaces.take_failure())
    {
        Err(error)
    } else {
        Ok(visible_trait_impls)
    }
}
