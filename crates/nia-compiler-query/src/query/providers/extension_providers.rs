// SPDX-License-Identifier: GPL-3.0-or-later
use super::program_signatures::program_signature_facts;
use super::*;

pub(super) fn provide_extension_provider_summary(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_provider_summary::ProviderSummary {
    let tree = db.query(SignatureItemTreeQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Traits,
    ));
    nia_provider_summary::ProviderSummary::from_active_item_tree(&tree)
}

pub(super) fn provide_extension_provider_module_ids(
    db: &QueryDb<CompilerContext>,
) -> Vec<ModuleId> {
    let timings = db.context().timings();
    let semantic_modules = db.query(SemanticModuleIdsQuery);
    time_provider(timings, "extension_provider_module_ids.filter", || {
        semantic_modules
            .into_iter()
            .filter(|module_id| db.query(ExtensionProviderModuleEligibilityQuery(*module_id)))
            .collect()
    })
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

pub(super) fn provide_extension_method_set(
    db: &QueryDb<CompilerContext>,
) -> ExtensionMethodSetValue {
    time_provider(db.context().timings(), "extension_method_set", || {
        let facts = db.query(ExtensionProviderValidationProgramFactsQuery);
        Arc::new(ExtensionMethodSetQueryValue {
            methods: facts.methods.clone(),
            diagnostics: facts.diagnostics.clone(),
        })
    })
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
                trait_impls: Vec::new(),
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
        let trait_impls = collect_valid_trait_impls_for_extension_index_module(&module);
        let nominal_providers =
            collect_nominal_extension_providers_for_module(&module, &module_defs);
        Arc::new(ExtensionProviderModuleFactsQueryValue {
            methods,
            associated_values,
            associated_value_diagnostics,
            trait_impls,
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
        let (methods, diagnostics) = collect_extension_methods_for_module(
            &input.module(),
            ExtensionMethodValidationInput {
                trait_defs: &trait_index.trait_defs,
                trait_signatures: &trait_index.trait_signatures,
                trait_impls: &trait_solving.trait_impls,
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

pub(super) fn provide_extension_provider_validation_program_facts(
    db: &QueryDb<CompilerContext>,
) -> ExtensionProviderValidationProgramFactsValue {
    time_provider(
        db.context().timings(),
        "extension_provider_validation_program_facts",
        || {
            let facts = db.query_many(
                db.query(ExtensionProviderModuleIdsQuery)
                    .into_iter()
                    .map(ExtensionProviderValidationFactsQuery),
            );
            let mut methods = nia_defs::ExtensionMethods::default();
            let mut diagnostics = Vec::new();
            for facts in facts {
                methods.extend(facts.methods.clone());
                diagnostics.extend(facts.diagnostics.iter().cloned());
            }
            Arc::new(ExtensionProviderValidationProgramFactsQueryValue {
                methods,
                diagnostics,
            })
        },
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

pub(super) fn provide_extension_provider_nominal_candidate_index(
    db: &QueryDb<CompilerContext>,
) -> ExtensionProviderNominalCandidateIndexValue {
    time_provider(
        db.context().timings(),
        "extension_provider_nominal_candidate_index",
        || {
            let index = nia_provider_summary::NominalProviderCandidateIndex::from_summaries(
                db.query(ExtensionProviderModuleIdsQuery)
                    .into_iter()
                    .map(|module_id| {
                        (
                            module_id,
                            db.query(ExtensionProviderSummaryQuery(module_id)),
                        )
                    }),
            );
            Arc::new(ExtensionProviderNominalCandidateIndexQueryValue(index))
        },
    )
}

pub(super) fn provide_extension_provider_nominal_index(
    db: &QueryDb<CompilerContext>,
) -> ExtensionProviderNominalIndexValue {
    time_provider(
        db.context().timings(),
        "extension_provider_nominal_index",
        || {
            let candidate_index = db.query(ExtensionProviderNominalCandidateIndexQuery);
            let candidate_index = &candidate_index.0;
            let mut module_ids = candidate_index.conservative().to_vec();
            module_ids.extend(candidate_index.all_named());
            module_ids.sort();
            module_ids.dedup();

            let mut providers_by_target: HashMap<
                GlobalDefId,
                Vec<crate::program_signatures::NominalExtensionProviderEntry>,
            > = HashMap::new();
            for facts in db.query_many(
                module_ids
                    .into_iter()
                    .map(ExtensionProviderNominalModuleFactsQuery),
            ) {
                for provider in &facts.nominal_providers {
                    providers_by_target
                        .entry(provider.target)
                        .or_default()
                        .push(*provider);
                }
            }
            for providers in providers_by_target.values_mut() {
                sort_and_dedup_nominal_providers(providers);
            }
            Arc::new(ExtensionProviderNominalIndexQueryValue {
                providers_by_target,
            })
        },
    )
}

pub(super) fn provide_extension_provider_nominal_modules(
    db: &QueryDb<CompilerContext>,
    target: GlobalDefId,
    accessing_module: ModuleId,
) -> ExtensionProviderNominalModulesValue {
    time_provider(
        db.context().timings(),
        "extension_provider_nominal_modules",
        || {
            let graph = db.query(ModuleGraphQuery);
            let nominal_index = db.query(ExtensionProviderNominalIndexQuery);
            let providers = nominal_index
                .providers_by_target
                .get(&target)
                .into_iter()
                .flat_map(|providers| providers.iter().copied())
                .filter(|provider| {
                    nia_imports::visibility_allows(
                        provider.visibility,
                        &graph,
                        provider.module_id,
                        accessing_module,
                    )
                })
                .collect::<Vec<_>>();
            let modules = providers
                .into_iter()
                .map(|provider| provider.module_id)
                .collect::<Vec<_>>();
            Arc::new(ExtensionProviderNominalModulesQueryValue { modules })
        },
    )
}

fn sort_and_dedup_nominal_providers(
    providers: &mut Vec<crate::program_signatures::NominalExtensionProviderEntry>,
) {
    providers.sort_by_key(|provider| {
        (
            provider.target,
            provider.module_id,
            match provider.visibility {
                nia_ids::Visibility::Private => 0,
                nia_ids::Visibility::PublicSuper => 1,
                nia_ids::Visibility::PublicPkg => 2,
                nia_ids::Visibility::Public => 3,
            },
        )
    });
    providers.dedup();
}

pub(super) fn provide_extension_method_index(
    db: &QueryDb<CompilerContext>,
) -> ExtensionMethodIndexValue {
    time_provider(db.context().timings(), "extension_method_index", || {
        let mut methods = nia_defs::ExtensionMethods::default();
        let mut trait_impls = Vec::new();
        for facts in extension_provider_module_facts(db) {
            methods.extend(facts.methods.clone());
            trait_impls.extend(facts.trait_impls.iter().cloned());
        }
        Arc::new(ExtensionMethodIndexQueryValue {
            methods,
            trait_impls,
        })
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

pub(super) fn provide_extension_associated_values(
    db: &QueryDb<CompilerContext>,
) -> ExtensionAssociatedValuesValue {
    time_provider(
        db.context().timings(),
        "extension_associated_values",
        || {
            let mut values = nia_defs::ExtensionAssociatedValues::default();
            let mut diagnostics = Vec::new();
            for facts in extension_provider_module_facts(db) {
                values.extend(facts.associated_values.clone());
                diagnostics.extend(facts.associated_value_diagnostics.iter().cloned());
            }
            Arc::new(ExtensionAssociatedValuesQueryValue {
                values,
                diagnostics,
            })
        },
    )
}

pub(super) fn provide_extension_methods(db: &QueryDb<CompilerContext>) -> ExtensionMethodsValue {
    time_provider(db.context().timings(), "extension_methods", || {
        let method_set = db.query(ExtensionMethodSetQuery);
        let associated_values = db.query(ExtensionAssociatedValuesQuery);
        let mut diagnostics = method_set.diagnostics.clone();
        diagnostics.extend(associated_values.diagnostics.iter().cloned());
        Arc::new(ExtensionMethodsQueryValue {
            methods: method_set.methods.clone(),
            associated_values: associated_values.values.clone(),
            diagnostics,
        })
    })
}

pub(super) fn provide_visible_extensions(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> VisibleExtensionsValue {
    let graph = db.query(ModuleGraphQuery);
    let defs = |module_id| Some(db.query(ModuleDefsQuery(module_id)));
    let public = db.query(PublicSurfaceQuery);
    let empty_using = ModuleUsingScope::default();
    let using_scope = public.using_scopes.get(&module_id).unwrap_or(&empty_using);
    let extension_method_normalization = |module_id| {
        Some(db.query(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        )))
    };
    let visible_type_signatures = db.query(ProgramVisibleTypeSignaturesQuery);
    let nominal_extension_providers = |target_def_id| {
        db.query(ExtensionProviderNominalModulesQuery(
            target_def_id,
            module_id,
        ))
        .modules
        .clone()
    };
    let provider_modules = crate::program_signatures::visible_extension_provider_modules(
        crate::program_signatures::VisibleExtensionProviderModulesInput {
            module_id,
            graph: &graph,
            using_scope,
            using_scopes: &public.using_scopes,
            defs: &defs,
            normalizations: &extension_method_normalization,
            visible_type_signatures: VisibleTypeSignatures {
                type_aliases: &visible_type_signatures.type_aliases,
            },
            nominal_extension_providers: &nominal_extension_providers,
        },
    );
    let mut extension_methods = nia_defs::ExtensionMethods::default();
    let mut associated_values = nia_defs::ExtensionAssociatedValues::default();
    let mut trait_impls = Vec::new();
    for provider_module in std::iter::once(module_id).chain(provider_modules.iter().copied()) {
        let facts = db.query(ExtensionProviderModuleFactsQuery(provider_module));
        extension_methods.extend(facts.methods.clone());
        associated_values.extend(facts.associated_values.clone());
        trait_impls.extend(facts.trait_impls.iter().cloned());
    }
    Arc::new(visible_extensions_for_module(VisibleExtensionsInput {
        module_id,
        graph: &graph,
        using_scope,
        using_scopes: &public.using_scopes,
        public_surfaces: &public.surfaces,
        defs: &defs,
        normalizations: &extension_method_normalization,
        visible_type_signatures: VisibleTypeSignatures {
            type_aliases: &visible_type_signatures.type_aliases,
        },
        extensions: &extension_methods,
        associated_values: &associated_values,
        trait_impls: trait_impls.as_slice(),
        nominal_extension_providers: &nominal_extension_providers,
        visible_modules: Some(provider_modules.as_slice()),
    }))
}
