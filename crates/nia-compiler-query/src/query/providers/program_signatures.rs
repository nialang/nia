// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn provide_program_signature_module_ids(
    db: &QueryDb<CompilerContext>,
    set: nia_item_tree::SignatureItemSet,
) -> Vec<ModuleId> {
    db.query(SemanticModuleIdsQuery)
        .into_iter()
        .filter(|module_id| db.query(ProgramSignatureModuleEligibilityQuery(*module_id, set)))
        .collect()
}

pub(super) fn provide_program_signature_module_eligibility(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> bool {
    let tree = db.query_shared(SignatureItemTreeQuery(module_id, set));
    nia_program_signatures::signature_tree_has_program_signature_facts(&tree, set)
}

pub(super) fn provide_module_program_signature_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> ModuleProgramSignatureFactsValue {
    let defs = db.query_shared(ModuleDefsQuery(module_id));
    let lowering = db.query_shared(SignatureTypeLoweringQuery(module_id, set));
    let signatures = db.query_shared(SignatureItemSignaturesQuery(module_id, set));
    Arc::new(
        nia_program_signatures::collect_module_program_signature_facts(ModuleSignatureInput {
            module_id,
            defs: &defs,
            lowering: &lowering,
            signatures: &signatures,
        }),
    )
}

pub(super) fn provide_module_abi_signature_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ModuleAbiSignatureFactsValue {
    let signatures = db.query_shared(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ));
    Arc::new(ModuleAbiSignatureFactsQueryValue {
        structs: signatures
            .structs
            .iter()
            .map(|(def_id, signature)| {
                (
                    GlobalDefId {
                        module_id,
                        def_id: *def_id,
                    },
                    signature.clone(),
                )
            })
            .collect(),
        unions: signatures
            .unions
            .iter()
            .map(|(def_id, signature)| {
                (
                    GlobalDefId {
                        module_id,
                        def_id: *def_id,
                    },
                    signature.clone(),
                )
            })
            .collect(),
        enums: signatures
            .enums
            .iter()
            .map(|(def_id, signature)| {
                (
                    GlobalDefId {
                        module_id,
                        def_id: *def_id,
                    },
                    signature.clone(),
                )
            })
            .collect(),
    })
}

pub(super) fn program_signature_facts(
    db: &QueryDb<CompilerContext>,
    set: nia_item_tree::SignatureItemSet,
) -> Vec<ModuleProgramSignatureFactsValue> {
    db.query_many(
        db.query(ProgramSignatureModuleIdsQuery(set))
            .into_iter()
            .map(|module_id| ModuleProgramSignatureFactsQuery(module_id, set)),
    )
}

fn collect_functions_excluding(
    facts: &[ModuleProgramSignatureFactsValue],
    excluded: &HashSet<GlobalDefId>,
) -> HashMap<GlobalDefId, ProgramFunctionSignature> {
    facts
        .iter()
        .flat_map(|facts| {
            facts
                .functions
                .iter()
                .filter(|(def_id, _)| !excluded.contains(def_id))
                .map(|(def_id, signature)| (*def_id, signature.clone()))
        })
        .collect()
}

fn collect_globals(
    facts: &[ModuleProgramSignatureFactsValue],
) -> HashMap<GlobalDefId, ProgramGlobalSignature> {
    facts
        .iter()
        .flat_map(|facts| {
            facts
                .globals
                .iter()
                .map(|(def_id, sig)| (*def_id, sig.clone()))
        })
        .collect()
}

fn collect_comptimes(
    facts: &[ModuleProgramSignatureFactsValue],
) -> HashMap<GlobalDefId, ProgramComptimeSignature> {
    facts
        .iter()
        .flat_map(|facts| {
            facts
                .comptimes
                .iter()
                .map(|(def_id, sig)| (*def_id, sig.clone()))
        })
        .collect()
}

fn collect_structs(
    facts: &[ModuleProgramSignatureFactsValue],
) -> HashMap<GlobalDefId, ProgramStructSignature> {
    facts
        .iter()
        .flat_map(|facts| {
            facts
                .structs
                .iter()
                .map(|(def_id, sig)| (*def_id, sig.clone()))
        })
        .collect()
}

fn collect_unions(
    facts: &[ModuleProgramSignatureFactsValue],
) -> HashMap<GlobalDefId, ProgramUnionSignature> {
    facts
        .iter()
        .flat_map(|facts| {
            facts
                .unions
                .iter()
                .map(|(def_id, sig)| (*def_id, sig.clone()))
        })
        .collect()
}

fn collect_enums(
    facts: &[ModuleProgramSignatureFactsValue],
) -> HashMap<GlobalDefId, ProgramEnumSignature> {
    facts
        .iter()
        .flat_map(|facts| {
            facts
                .enums
                .iter()
                .map(|(def_id, sig)| (*def_id, sig.clone()))
        })
        .collect()
}

fn collect_traits(
    facts: &[ModuleProgramSignatureFactsValue],
) -> HashMap<GlobalDefId, ProgramTraitSignature> {
    facts
        .iter()
        .flat_map(|facts| {
            facts
                .traits
                .iter()
                .map(|(def_id, sig)| (*def_id, sig.clone()))
        })
        .collect()
}

fn collect_trait_method_index(
    facts: &[ModuleProgramSignatureFactsValue],
) -> ProgramTraitMethodIndex {
    ProgramTraitMethodIndex::from_trait_signatures(
        facts
            .iter()
            .flat_map(|facts| facts.traits.iter().map(|(def_id, sig)| (*def_id, sig))),
    )
}

fn collect_type_aliases(
    facts: &[ModuleProgramSignatureFactsValue],
) -> HashMap<GlobalDefId, ProgramTypeAliasSignature> {
    facts
        .iter()
        .flat_map(|facts| {
            facts
                .type_aliases
                .iter()
                .map(|(def_id, sig)| (*def_id, sig.clone()))
        })
        .collect()
}

fn collect_trait_impls(
    facts: &[ModuleProgramSignatureFactsValue],
) -> Vec<nia_item_signatures::ProgramTraitImplSignature> {
    facts
        .iter()
        .flat_map(|facts| facts.trait_impls.iter().cloned())
        .collect()
}

pub(super) fn provide_program_visible_type_signatures(
    db: &QueryDb<CompilerContext>,
) -> Arc<ProgramVisibleTypeSignatures> {
    time_provider(
        db.context().timings(),
        "program_visible_type_signatures",
        || {
            let facts = program_signature_facts(db, nia_item_tree::SignatureItemSet::Types);
            Arc::new(ProgramVisibleTypeSignatures {
                type_aliases: collect_type_aliases(&facts),
            })
        },
    )
}

pub(super) fn provide_program_trait_method_index(
    db: &QueryDb<CompilerContext>,
) -> Arc<ProgramTraitMethodIndex> {
    time_provider(db.context().timings(), "program_trait_method_index", || {
        let trait_facts = program_signature_facts(db, nia_item_tree::SignatureItemSet::Traits);
        Arc::new(collect_trait_method_index(&trait_facts))
    })
}

pub(super) fn executable_program_signatures_without_functions(
    db: &QueryDb<CompilerContext>,
) -> ProgramExecutableSignatures {
    let value_facts = program_signature_facts(db, nia_item_tree::SignatureItemSet::Values);
    let type_facts = program_signature_facts(db, nia_item_tree::SignatureItemSet::Types);
    let trait_facts = program_signature_facts(db, nia_item_tree::SignatureItemSet::Traits);
    let traits = collect_traits(&trait_facts);
    let trait_method_index = ProgramTraitMethodIndex::from_traits(&traits);
    let trait_impls = collect_trait_impls(&trait_facts);
    let trait_impl_index = nia_item_signatures::ProgramTraitImplIndex::new(&trait_impls);
    ProgramExecutableSignatures {
        functions: HashMap::new(),
        globals: collect_globals(&value_facts),
        comptimes: collect_comptimes(&value_facts),
        structs: collect_structs(&type_facts),
        unions: collect_unions(&type_facts),
        enums: collect_enums(&type_facts),
        type_aliases: collect_type_aliases(&type_facts),
        traits,
        trait_impls,
        trait_impl_index,
        trait_method_index,
    }
}

pub(super) fn executable_program_functions_for_modules(
    db: &QueryDb<CompilerContext>,
    module_ids: impl IntoIterator<Item = ModuleId>,
) -> HashMap<GlobalDefId, ProgramFunctionSignature> {
    module_ids
        .into_iter()
        .flat_map(|module_id| {
            let lowered = db.query(TypeLoweringQuery(module_id));
            let signatures = body_local_item_signatures(db, module_id, &lowered);
            let defs = db.query_shared(ModuleDefsQuery(module_id));
            let interner = lowered.interner;
            signatures
                .functions
                .into_iter()
                .map(move |(def_id, signature)| {
                    let global_def_id = GlobalDefId { module_id, def_id };
                    let name = defs
                        .defs
                        .get(def_id)
                        .map(|def| def.name.clone())
                        .unwrap_or_default();
                    (
                        global_def_id,
                        ProgramFunctionSignature {
                            name,
                            signature,
                            interner: interner.clone(),
                        },
                    )
                })
        })
        .collect()
}

pub(super) fn provide_program_backend_signatures(
    db: &QueryDb<CompilerContext>,
) -> Arc<ProgramBackendSignatures> {
    time_provider(db.context().timings(), "program_backend_signatures", || {
        let function_facts =
            program_signature_facts(db, nia_item_tree::SignatureItemSet::Functions);
        let type_facts = program_signature_facts(db, nia_item_tree::SignatureItemSet::Types);
        let trait_facts = program_signature_facts(db, nia_item_tree::SignatureItemSet::Traits);
        let trait_solving = db.query(ProgramTraitSolvingSignaturesQuery);
        let traits = collect_traits(&trait_facts);
        Arc::new(ProgramBackendSignatures {
            functions: collect_functions_excluding(
                &function_facts,
                &trait_solving.invalid_trait_impl_method_ids,
            ),
            structs: collect_structs(&type_facts),
            unions: collect_unions(&type_facts),
            enums: collect_enums(&type_facts),
            traits,
            type_aliases: collect_type_aliases(&type_facts),
            trait_impls: trait_solving.trait_impls.clone(),
            trait_impl_index: trait_solving.trait_impl_index.clone(),
        })
    })
}

pub(super) fn provide_program_abi_signatures(
    db: &QueryDb<CompilerContext>,
) -> Arc<ProgramAbiSignaturesValue> {
    time_provider(db.context().timings(), "program_abi_signatures", || {
        let facts = db.query_many(
            db.query(ProgramSignatureModuleIdsQuery(
                nia_item_tree::SignatureItemSet::Types,
            ))
            .into_iter()
            .map(ModuleAbiSignatureFactsQuery),
        );
        let mut structs = HashMap::new();
        let mut unions = HashMap::new();
        let mut enums = HashMap::new();
        for facts in facts {
            structs.extend(
                facts
                    .structs
                    .iter()
                    .map(|(def_id, signature)| (*def_id, signature.clone())),
            );
            unions.extend(
                facts
                    .unions
                    .iter()
                    .map(|(def_id, signature)| (*def_id, signature.clone())),
            );
            enums.extend(
                facts
                    .enums
                    .iter()
                    .map(|(def_id, signature)| (*def_id, signature.clone())),
            );
        }
        Arc::new(ProgramAbiSignaturesValue {
            structs,
            unions,
            enums,
        })
    })
}
