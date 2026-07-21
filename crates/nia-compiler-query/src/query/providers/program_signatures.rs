// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn provide_program_signature_module_ids(
    db: &QueryDb<CompilerContext>,
    set: nia_item_tree::SignatureItemSet,
) -> StableModuleSequence {
    let module_ids =
        resolve_stable_module_sequence_from_current_inputs(db, &db.get(SemanticModuleIdsQuery));
    let eligible = module_ids
        .into_iter()
        .filter(|module_id| *db.get(ProgramSignatureModuleEligibilityQuery(*module_id, set)))
        .collect::<Vec<_>>();
    stable_module_sequence(db, eligible)
}

pub(super) fn provide_program_signature_module_eligibility(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> bool {
    let tree = db.get(SignatureItemTreeQuery(module_id, set));
    nia_program_signatures::signature_tree_has_program_signature_facts(&tree, set)
}

pub(super) fn provide_module_program_signature_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> ModuleProgramSignatureFactsValue {
    let defs = db.get(ModuleDefsQuery(module_id));
    let lowering = db.get(SignatureTypeLoweringQuery(module_id, set));
    let signatures = db.get(SignatureItemSignaturesQuery(module_id, set));
    nia_program_signatures::collect_module_program_signature_facts(ModuleSignatureInput {
        module_id,
        type_store: &db.context().type_store,
        defs: &defs,
        lowering: &lowering,
        signatures: &signatures,
    })
}

pub(super) fn provide_module_abi_signature_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ModuleAbiSignatureFactsValue {
    let signatures = db.get(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ));
    ModuleAbiSignatureFactsQueryValue {
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
    }
}

pub(super) fn program_signature_facts(
    db: &QueryDb<CompilerContext>,
    set: nia_item_tree::SignatureItemSet,
) -> Vec<Arc<ModuleProgramSignatureFactsValue>> {
    let module_ids =
        resolve_stable_module_sequence(db, &db.get(ProgramSignatureModuleIdsQuery(set)));
    db.get_many(
        module_ids
            .into_iter()
            .map(|module_id| ModuleProgramSignatureFactsQuery(module_id, set)),
    )
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
) -> ProgramTraitMethodIndex {
    time_provider(db.context().timings(), "program_trait_method_index", || {
        let trait_facts = program_signature_facts(db, nia_item_tree::SignatureItemSet::Traits);
        collect_trait_method_index(&trait_facts)
    })
}

pub(super) fn executable_program_non_function_signatures(
    db: &QueryDb<CompilerContext>,
) -> ProgramExecutableNonFunctionSignatures {
    let value_facts = program_signature_facts(db, nia_item_tree::SignatureItemSet::Values);
    let type_facts = program_signature_facts(db, nia_item_tree::SignatureItemSet::Types);
    let trait_facts = program_signature_facts(db, nia_item_tree::SignatureItemSet::Traits);
    let traits = collect_traits(&trait_facts);
    let trait_method_index = ProgramTraitMethodIndex::from_traits(&traits);
    let trait_impls = collect_trait_impls(&trait_facts);
    let trait_impl_index = nia_item_signatures::ProgramTraitImplIndex::new(&trait_impls);
    ProgramExecutableNonFunctionSignatures {
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
    }
}

pub(super) fn executable_program_functions_for_modules(
    db: &QueryDb<CompilerContext>,
    module_ids: impl IntoIterator<Item = ModuleId>,
) -> HashMap<GlobalDefId, ProgramFunctionSignature> {
    module_ids
        .into_iter()
        .flat_map(|module_id| {
            let lowered = db.get(TypeLoweringQuery(module_id));
            let signatures = body_local_item_signatures(db, module_id, &lowered);
            let defs = db.get(ModuleDefsQuery(module_id));
            signatures
                .functions
                .into_iter()
                .map(move |(def_id, signature)| {
                    let global_def_id = GlobalDefId { module_id, def_id };
                    let name = defs
                        .defs
                        .get(def_id)
                        .map(|def| def.name)
                        .unwrap_or_default();
                    (global_def_id, ProgramFunctionSignature { name, signature })
                })
        })
        .collect()
}

pub(super) fn provide_program_abi_signatures(
    db: &QueryDb<CompilerContext>,
) -> ProgramAbiSignaturesValue {
    time_provider(db.context().timings(), "program_abi_signatures", || {
        let module_ids = resolve_stable_module_sequence(
            db,
            &db.get(ProgramSignatureModuleIdsQuery(
                nia_item_tree::SignatureItemSet::Types,
            )),
        );
        let facts = db.get_many(module_ids.into_iter().map(ModuleAbiSignatureFactsQuery));
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
        ProgramAbiSignaturesValue {
            structs,
            unions,
            enums,
        }
    })
}
