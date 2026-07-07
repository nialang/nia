// SPDX-License-Identifier: GPL-3.0-or-later
use super::program_signatures::program_signature_facts;
use super::*;

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
    let trait_modules = db.query(ProgramSignatureModuleIdsQuery(
        nia_item_tree::SignatureItemSet::Traits,
    ));
    time_provider(timings, "extension_provider_discovery_index", || {
        let mut provider_modules = Vec::new();
        let mut nominal_candidates_by_name: HashMap<SymbolId, Vec<ModuleId>> = HashMap::new();
        for module_id in trait_modules {
            let summary = db.query(ExtensionProviderSummaryQuery(module_id));
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
        }
        provider_modules.sort();
        provider_modules.dedup();
        for modules in nominal_candidates_by_name.values_mut() {
            modules.sort();
            modules.dedup();
        }
        Arc::new(ExtensionProviderDiscoveryIndexQueryValue {
            provider_modules,
            nominal_candidates_by_name,
        })
    })
}

pub(super) fn provide_extension_provider_module_ids(
    db: &QueryDb<CompilerContext>,
) -> Vec<ModuleId> {
    db.query(ExtensionProviderDiscoveryIndexQuery)
        .provider_modules
        .clone()
}

pub(super) fn provide_extension_provider_module_eligibility(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> bool {
    db.query(ExtensionProviderSummaryQuery(module_id))
        .has_providers()
}

pub(super) fn provide_extension_signature_module_input(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ExtensionSignatureModuleInputValue {
    Arc::new(ExtensionSignatureModuleInputQueryValue {
        module_id,
        defs: db.query(ModuleDefsQuery(module_id)),
        lowering: db.query(SignatureTypeLoweringQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        )),
        signatures: db.query(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        )),
        function_signatures: db.query(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::ExtensionFunctions,
        )),
        type_signatures: db.query(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Types,
        )),
        normalization: db.query(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        )),
    })
}

pub(super) fn provide_extension_trait_solving_module_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ExtensionTraitSolvingModuleFactsValue {
    let input = db.query(ExtensionSignatureModuleInputQuery(module_id));
    let modules = [input.module()];
    Arc::new(ExtensionTraitSolvingModuleFactsQueryValue {
        trait_impls: crate::program_signatures::collect_valid_program_trait_impls(&modules),
        invalid_trait_impl_method_ids:
            crate::program_signatures::collect_invalid_trait_impl_method_ids(&modules),
    })
}

pub(super) fn provide_program_trait_solving_signatures(
    db: &QueryDb<CompilerContext>,
) -> Arc<ProgramTraitSolvingSignatures> {
    time_provider(
        db.context().timings(),
        "program_trait_solving_signatures",
        || {
            let facts = db.query_many(
                db.query(ExtensionProviderModuleIdsQuery)
                    .into_iter()
                    .map(ExtensionTraitSolvingModuleFactsQuery),
            );
            let mut trait_impls = Vec::new();
            let mut invalid_trait_impl_method_ids = HashSet::new();
            for facts in &facts {
                trait_impls.extend(facts.trait_impls.iter().cloned());
                invalid_trait_impl_method_ids
                    .extend(facts.invalid_trait_impl_method_ids.iter().copied());
            }
            Arc::new(ProgramTraitSolvingSignatures {
                trait_impls,
                invalid_trait_impl_method_ids,
            })
        },
    )
}

pub(super) fn provide_extension_provider_module_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ExtensionProviderModuleFactsValue {
    time_module_provider(db, "extension_provider_module_facts", module_id, || {
        if !db.query(ExtensionProviderModuleEligibilityQuery(module_id)) {
            return Arc::new(ExtensionProviderModuleFactsQueryValue {
                methods: nia_defs::ExtensionMethods::default(),
                associated_values: nia_defs::ExtensionAssociatedValues::default(),
                associated_value_diagnostics: Vec::new(),
                nominal_providers: Vec::new(),
            });
        }

        let defs = db.query(ModuleDefsQuery(module_id));
        let lowering = db.query(SignatureTypeLoweringQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let signatures = db.query(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let normalization = db.query(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let module = ExtensionMethodIndexModuleInput {
            module_id,
            defs: &defs,
            lowering: &lowering,
            signatures: &signatures,
            normalization: &normalization,
        };
        let module_defs = |module_id| Some(db.query(ModuleDefsQuery(module_id)));
        let methods = collect_extension_method_index_for_module(&module, &module_defs);
        let (associated_values, associated_value_diagnostics) =
            collect_extension_associated_value_index_for_module(&module);
        let nominal_providers =
            collect_nominal_extension_providers_for_module(&module, &module_defs);
        Arc::new(ExtensionProviderModuleFactsQueryValue {
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
) -> ExtensionProviderValidationFactsValue {
    time_module_provider(db, "extension_provider_validation_facts", module_id, || {
        if !db.query(ExtensionProviderModuleEligibilityQuery(module_id)) {
            return Arc::new(ExtensionProviderValidationFactsQueryValue {
                methods: nia_defs::ExtensionMethods::default(),
                diagnostics: Vec::new(),
            });
        }
        let input = db.query(ExtensionSignatureModuleInputQuery(module_id));
        let trait_index = db.query(ExtensionTraitSignatureIndexQuery);
        let trait_solving = db.query(ProgramTraitSolvingSignaturesQuery);
        let symbols = db.context().symbols();
        let (methods, diagnostics) = collect_extension_methods_for_module(
            &input.module(),
            ExtensionMethodValidationInput {
                trait_defs: &trait_index.trait_defs,
                trait_signatures: &trait_index.trait_signatures,
                trait_impls: &trait_solving.trait_impls,
                symbols: &symbols,
            },
        );
        Arc::new(ExtensionProviderValidationFactsQueryValue {
            methods,
            diagnostics,
        })
    })
}

fn extension_provider_module_facts(
    db: &QueryDb<CompilerContext>,
) -> Vec<ExtensionProviderModuleFactsValue> {
    db.query_many(
        db.query(ExtensionProviderModuleIdsQuery)
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
            if !db.query(ExtensionProviderModuleEligibilityQuery(module_id)) {
                return Arc::new(ExtensionProviderNominalModuleFactsQueryValue {
                    nominal_providers: Vec::new(),
                });
            }

            let defs = db.query(ModuleDefsQuery(module_id));
            let lowering = db.query(SignatureTypeLoweringQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Traits,
            ));
            let signatures = db.query(SignatureItemSignaturesQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Traits,
            ));
            let normalization = db.query(SignatureTypeNormalizationQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Traits,
            ));
            let module = ExtensionMethodIndexModuleInput {
                module_id,
                defs: &defs,
                lowering: &lowering,
                signatures: &signatures,
                normalization: &normalization,
            };
            let module_defs = |module_id| Some(db.query(ModuleDefsQuery(module_id)));
            let nominal_providers =
                collect_nominal_extension_providers_for_module(&module, &module_defs);
            Arc::new(ExtensionProviderNominalModuleFactsQueryValue { nominal_providers })
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
            let discovery = db.query(ExtensionProviderDiscoveryIndexQuery);
            let mut modules = Vec::new();
            for name in names.0 {
                if let Some(candidates) = discovery.nominal_candidates_by_name.get(&name) {
                    modules.extend(candidates.iter().copied());
                }
            }
            modules.sort();
            modules.dedup();
            Arc::new(ExtensionProviderNominalCandidateModulesQueryValue { modules })
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
            let graph = db.query(ModuleGraphQuery);
            let index_names = extension_provider_nominal_target_names_for_targets(db, &targets);
            let candidate_modules = db
                .query(ExtensionProviderNominalCandidateModulesQuery(
                    ExtensionProviderNominalTargetNames::new(index_names),
                ))
                .modules
                .clone();
            let mut modules = Vec::new();
            for facts in db.query_many(
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
            Arc::new(ExtensionProviderNominalModulesForTargetsQueryValue { modules })
        },
    )
}

fn extension_provider_nominal_target_names_for_targets(
    db: &QueryDb<CompilerContext>,
    targets: &ExtensionProviderNominalTargets,
) -> Vec<SymbolId> {
    let type_exposures = db.query(TypeExposureIndexQuery);
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
        Arc::new(ExtensionMethodIndexQueryValue { methods })
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
            Arc::new(ExtensionTraitSignatureIndex {
                trait_defs,
                trait_signatures,
            })
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
        crate::program_signatures::visible_extension_provider_modules,
    )
}

fn visible_trait_impl_modules_for_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> Vec<ModuleId> {
    visible_modules_for_module(
        db,
        module_id,
        crate::program_signatures::visible_trait_impl_modules,
    )
}

fn visible_modules_for_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    compute: fn(
        crate::program_signatures::VisibleExtensionProviderModulesInput<'_>,
    ) -> Vec<ModuleId>,
) -> Vec<ModuleId> {
    let graph = db.query(ModuleGraphQuery);
    let defs = |module_id| Some(db.query(ModuleDefsQuery(module_id)));
    let public_using_scopes = db.query(PublicUsingScopesQuery);
    let using_scope = db.query(ModuleUsingScopeQuery(module_id));
    let extension_method_normalization = |module_id| {
        Some(db.query(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        )))
    };
    let visible_type_signatures = db.query(ProgramVisibleTypeSignaturesQuery);
    let nominal_extension_providers = |target_def_ids: &[GlobalDefId]| {
        db.query(ExtensionProviderNominalModulesForTargetsQuery(
            ExtensionProviderNominalTargets::new(target_def_ids.to_vec()),
            module_id,
        ))
        .modules
        .clone()
    };
    let provider_modules = compute(
        crate::program_signatures::VisibleExtensionProviderModulesInput {
            module_id,
            graph: &graph,
            using_scope: &using_scope,
            using_scopes: &public_using_scopes.using_scopes,
            defs: &defs,
            normalizations: &extension_method_normalization,
            visible_type_signatures: VisibleTypeSignatures {
                type_aliases: &visible_type_signatures.type_aliases,
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
    let graph = db.query(ModuleGraphQuery);
    let defs = |module_id| Some(db.query(ModuleDefsQuery(module_id)));
    let public_surfaces = db.query(PublicSurfacesQuery);
    let public_using_scopes = db.query(PublicUsingScopesQuery);
    let using_scope = db.query(ModuleUsingScopeQuery(module_id));
    let extension_method_normalization = |module_id| {
        Some(db.query(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        )))
    };
    let visible_type_signatures = db.query(ProgramVisibleTypeSignaturesQuery);
    let nominal_extension_providers = |target_def_ids: &[GlobalDefId]| {
        db.query(ExtensionProviderNominalModulesForTargetsQuery(
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
        let facts = db.query(ExtensionProviderModuleFactsQuery(provider_module));
        extension_methods.extend(facts.methods.clone());
        associated_values.extend(facts.associated_values.clone());
    }
    Arc::new(visible_extensions_for_module(VisibleExtensionsInput {
        module_id,
        graph: &graph,
        using_scope: &using_scope,
        using_scopes: &public_using_scopes.using_scopes,
        public_surfaces: &public_surfaces.surfaces,
        defs: &defs,
        normalizations: &extension_method_normalization,
        visible_type_signatures: VisibleTypeSignatures {
            type_aliases: &visible_type_signatures.type_aliases,
        },
        extensions: &extension_methods,
        associated_values: &associated_values,
        trait_impls: &[],
        nominal_extension_providers: &nominal_extension_providers,
        visible_modules: Some(visible_modules.as_slice()),
    }))
}

pub(super) fn provide_visible_trait_impls(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> VisibleTraitImplsValue {
    let graph = db.query(ModuleGraphQuery);
    let defs = |module_id| Some(db.query(ModuleDefsQuery(module_id)));
    let public_surfaces = db.query(PublicSurfacesQuery);
    let public_using_scopes = db.query(PublicUsingScopesQuery);
    let using_scope = db.query(ModuleUsingScopeQuery(module_id));
    let extension_method_normalization = |module_id| {
        Some(db.query(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        )))
    };
    let visible_type_signatures = db.query(ProgramVisibleTypeSignaturesQuery);
    let nominal_extension_providers = |target_def_ids: &[GlobalDefId]| {
        db.query(ExtensionProviderNominalModulesForTargetsQuery(
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
            db.query(ExtensionTraitSolvingModuleFactsQuery(provider_module))
                .trait_impls
                .iter()
                .cloned(),
        );
    }
    Arc::new(visible_trait_impls_for_module(VisibleExtensionsInput {
        module_id,
        graph: &graph,
        using_scope: &using_scope,
        using_scopes: &public_using_scopes.using_scopes,
        public_surfaces: &public_surfaces.surfaces,
        defs: &defs,
        normalizations: &extension_method_normalization,
        visible_type_signatures: VisibleTypeSignatures {
            type_aliases: &visible_type_signatures.type_aliases,
        },
        extensions: &nia_defs::ExtensionMethods::default(),
        associated_values: &nia_defs::ExtensionAssociatedValues::default(),
        trait_impls: trait_impls.as_slice(),
        nominal_extension_providers: &nominal_extension_providers,
        visible_modules: Some(visible_modules.as_slice()),
    }))
}
