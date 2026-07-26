// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn provide_program_signature_module_ids(
    db: &QueryDb<CompilerContext>,
    set: nia_item_tree::SignatureItemSet,
) -> QueryResult<StableModuleSequence> {
    let semantic_modules = db.try_get(SemanticModuleIdsQuery)?;
    let module_ids = resolve_stable_module_sequence_from_current_inputs(db, &semantic_modules);
    let mut eligible = Vec::new();
    for module_id in module_ids {
        if *db.try_get(ProgramSignatureModuleEligibilityQuery(module_id, set))? {
            eligible.push(module_id);
        }
    }
    Ok(stable_module_sequence(db, eligible))
}

pub(super) fn provide_program_signature_module_eligibility(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> QueryResult<bool> {
    let tree = db.try_get(SignatureItemTreeQuery(module_id, set))?;
    Ok(nia_program_signatures::signature_tree_has_program_signature_facts(&tree, set))
}

pub(super) fn provide_module_program_signature_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> QueryResult<ModuleProgramSignatureFactsValue> {
    let signatures = db.try_get(SignatureItemSignaturesQuery(module_id, set))?;
    Ok(
        nia_program_signatures::collect_module_program_signature_facts(ModuleSignatureInput {
            module_id,
            type_store: &db.context().type_store,
            signatures: &signatures,
        }),
    )
}

pub(super) fn provide_module_abi_signature_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ModuleAbiSignatureFactsValue> {
    let signatures = db.try_get(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ))?;
    Ok(ModuleAbiSignatureFactsQueryValue {
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
) -> QueryResult<Vec<Arc<ModuleProgramSignatureFactsValue>>> {
    let module_sequence = db.try_get(ProgramSignatureModuleIdsQuery(set))?;
    let module_ids = resolve_stable_module_sequence(db, &module_sequence)?;
    module_ids
        .into_iter()
        .map(|module_id| db.try_get(ModuleProgramSignatureFactsQuery(module_id, set)))
        .collect()
}

fn collect_globals(
    facts: &[Arc<ModuleProgramSignatureFactsValue>],
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

fn collect_consts(
    facts: &[Arc<ModuleProgramSignatureFactsValue>],
) -> HashMap<GlobalDefId, ProgramConstSignature> {
    facts
        .iter()
        .flat_map(|facts| {
            facts
                .consts
                .iter()
                .map(|(def_id, sig)| (*def_id, sig.clone()))
        })
        .collect()
}

fn collect_structs(
    facts: &[Arc<ModuleProgramSignatureFactsValue>],
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
    facts: &[Arc<ModuleProgramSignatureFactsValue>],
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
    facts: &[Arc<ModuleProgramSignatureFactsValue>],
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
    facts: &[Arc<ModuleProgramSignatureFactsValue>],
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
    facts: &[Arc<ModuleProgramSignatureFactsValue>],
) -> ProgramTraitMethodIndex {
    ProgramTraitMethodIndex::from_trait_signatures(
        facts
            .iter()
            .flat_map(|facts| facts.traits.iter().map(|(def_id, sig)| (*def_id, sig))),
    )
}

fn collect_type_aliases(
    facts: &[Arc<ModuleProgramSignatureFactsValue>],
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
    facts: &[Arc<ModuleProgramSignatureFactsValue>],
) -> Vec<nia_item_signatures::ProgramTraitImplSignature> {
    facts
        .iter()
        .flat_map(|facts| facts.trait_impls.iter().cloned())
        .collect()
}

pub(super) fn provide_program_trait_method_index(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<ProgramTraitMethodIndex> {
    time_provider(db.context().timings(), "program_trait_method_index", || {
        let trait_facts = program_signature_facts(db, nia_item_tree::SignatureItemSet::Traits)?;
        Ok(collect_trait_method_index(&trait_facts))
    })
}

pub(super) fn executable_program_non_function_signatures(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<ProgramExecutableNonFunctionSignatures> {
    let value_facts = program_signature_facts(db, nia_item_tree::SignatureItemSet::Values)?;
    let type_facts = program_signature_facts(db, nia_item_tree::SignatureItemSet::Types)?;
    let trait_facts = program_signature_facts(db, nia_item_tree::SignatureItemSet::Traits)?;
    let traits = collect_traits(&trait_facts);
    let trait_method_index = ProgramTraitMethodIndex::from_traits(&traits);
    let trait_impls = collect_trait_impls(&trait_facts);
    let trait_impl_index = nia_item_signatures::ProgramTraitImplIndex::new(&trait_impls);
    Ok(ProgramExecutableNonFunctionSignatures {
        globals: collect_globals(&value_facts),
        consts: collect_consts(&value_facts),
        structs: collect_structs(&type_facts),
        unions: collect_unions(&type_facts),
        enums: collect_enums(&type_facts),
        type_aliases: collect_type_aliases(&type_facts),
        traits,
        trait_impls,
        trait_impl_index,
        trait_method_index,
    })
}

pub(super) fn executable_program_functions_for_modules(
    db: &QueryDb<CompilerContext>,
    module_ids: impl IntoIterator<Item = ModuleId>,
) -> QueryResult<HashMap<GlobalDefId, ProgramFunctionSignature>> {
    module_ids
        .into_iter()
        .map(|module_id| {
            let lowered = db.try_get(TypeLoweringQuery(module_id))?;
            let signatures = body_local_item_signatures(db, module_id, &lowered)?;
            Ok(signatures
                .functions
                .into_iter()
                .map(move |(def_id, signature)| {
                    let global_def_id = GlobalDefId { module_id, def_id };
                    let name = signature.name;
                    (global_def_id, ProgramFunctionSignature { name, signature })
                })
                .collect::<HashMap<_, _>>())
        })
        .collect::<QueryResult<Vec<_>>>()
        .map(|maps| maps.into_iter().flatten().collect())
}

pub(super) fn provide_program_abi_signatures(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<ProgramAbiSignaturesValue> {
    time_provider(db.context().timings(), "program_abi_signatures", || {
        let module_sequence = db.try_get(ProgramSignatureModuleIdsQuery(
            nia_item_tree::SignatureItemSet::Types,
        ))?;
        let module_ids = resolve_stable_module_sequence(db, &module_sequence)?;
        let facts = module_ids
            .into_iter()
            .map(|module_id| db.try_get(ModuleAbiSignatureFactsQuery(module_id)))
            .collect::<QueryResult<Vec<_>>>()?;
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
        Ok(ProgramAbiSignaturesValue {
            structs,
            unions,
            enums,
        })
    })
}
